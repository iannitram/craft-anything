//! A PE file's Authenticode signature block: the certificate table (whose directory entry is a
//! file offset, not an RVA), the PKCS#7 inside it, the signer, and what can be checked with the
//! file alone: its hash against the signed one, the signature, and how the embedded certificates
//! link. Whether Windows would trust the signer, revocation and catalog signing are out of reach.

use crate::der::{self, CertInfo};
use crate::pe::PeHeaders;
use crate::pkcs7;
use crate::verify::{self, ChainResult, Digest, Verified};
use serde::Serialize;

/// Most entries read from one table.
const MAX_ENTRIES: usize = 64;
/// Index of the security directory.
const SECURITY_DIR: usize = 4;
/// `WIN_CERT_TYPE_PKCS_SIGNED_DATA`.
const CERT_PKCS_SIGNED: u16 = 2;
const SPC_INDIRECT_DATA: &str = "1.3.6.1.4.1.311.2.1.4";

/// The certificate table and what it holds.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CertTable {
    pub file_offset: u32,
    pub size: u32,
    /// The whole table lies inside the file.
    pub within_file: bool,
    pub entries: Vec<CertEntry>,
    pub warnings: Vec<String>,
}

/// One `WIN_CERTIFICATE`.
#[derive(Debug, Clone, Serialize)]
pub struct CertEntry {
    pub file_offset: u64,
    pub length: u32,
    pub revision: u16,
    pub certificate_type: u16,
    pub type_name: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<SignatureReport>,
}

/// The file's hash compared with the one the signature covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HashStatus {
    Matches,
    Differs,
    NotChecked,
}

/// What a PKCS#7 signature block says and what was checked of it.
#[derive(Debug, Clone, Serialize)]
pub struct SignatureReport {
    /// Not well-formed `SignedData`: nothing below can be trusted.
    pub parsed: bool,
    pub signer: Option<CertInfo>,
    pub signer_count: usize,
    pub certificates: Vec<CertInfo>,
    /// The signer's digest algorithm, by name or dotted number.
    pub digest_algorithm: Option<String>,
    pub signature_algorithm: Option<String>,
    /// The algorithm and value of the file hash the signature covers.
    pub file_digest_algorithm: Option<String>,
    pub file_digest: Option<String>,
    pub computed_file_digest: Option<String>,
    pub file_hash: HashStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_hash_note: Option<String>,
    /// The signature, checked against the signer's embedded certificate.
    pub signature_check: Verified,
    /// How the embedded certificates link to one another. No trust store is consulted.
    pub chain: Option<ChainResult>,
    /// Things attached outside the signature, such as timestamps and nested signatures. Listed, not read.
    pub unsigned_attributes: Vec<String>,
    pub notes: Vec<String>,
}

fn type_name(t: u16) -> &'static str {
    match t {
        1 => "X.509 certificate",
        CERT_PKCS_SIGNED => "PKCS#7 signature (Authenticode)",
        3 => "reserved",
        4 => "terminal server protocol stack",
        _ => "unknown",
    }
}

/// Read the certificate table; `None` when the security directory is empty.
pub fn certificate_table(data: &[u8], h: &PeHeaders) -> Option<CertTable> {
    let (offset, size) = h.dir(SECURITY_DIR)?;
    let mut out = CertTable { file_offset: offset, size, ..CertTable::default() };
    let end = u64::from(offset).saturating_add(u64::from(size));
    out.within_file = end <= data.len() as u64;
    if !out.within_file {
        out.warnings.push(format!("the certificate table says it ends at byte {end}, but the file has {} bytes", data.len()));
    }
    let limit = end.min(data.len() as u64);
    let mut pos = u64::from(offset);
    while out.entries.len() < MAX_ENTRIES {
        let Some(header_end) = pos.checked_add(8).filter(|e| *e <= limit) else { break };
        let (Some(at), Some(body)) = (usize::try_from(pos).ok(), usize::try_from(header_end).ok()) else { break };
        let (Some(length), Some(revision), Some(certificate_type)) = (read_u32(data, at), read_u16(data, at + 4), read_u16(data, at + 6)) else { break };
        if length < 8 {
            out.warnings.push(format!("the entry at byte {pos} has a length of {length}, too short to be real"));
            break;
        }
        let Some(entry_end) = pos.checked_add(u64::from(length)).filter(|e| *e <= limit) else {
            out.warnings.push(format!("the entry at byte {pos} runs past the end of the table"));
            break;
        };
        let payload = usize::try_from(entry_end).ok().and_then(|e| data.get(body..e)).unwrap_or(&[]);
        let signature = (certificate_type == CERT_PKCS_SIGNED).then(|| read_signature(data, h, payload, out.within_file));
        out.entries.push(CertEntry { file_offset: pos, length, revision, certificate_type, type_name: type_name(certificate_type), signature });
        // Entries start on eight-byte boundaries.
        let Some(next) = entry_end.checked_next_multiple_of(8) else { break };
        pos = next;
    }
    Some(out)
}

fn read_u16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o.checked_add(2)?)?.try_into().ok()?))
}

fn read_u32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o.checked_add(4)?)?.try_into().ok()?))
}

fn hexs(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn named_oid(o: &str) -> String {
    der::oid_name(o).map_or_else(|| o.to_string(), str::to_string)
}

fn digest_name(d: Digest) -> String {
    d.name().to_string()
}

fn unparsed(note: &str) -> SignatureReport {
    SignatureReport {
        parsed: false,
        signer: None,
        signer_count: 0,
        certificates: Vec::new(),
        digest_algorithm: None,
        signature_algorithm: None,
        file_digest_algorithm: None,
        file_digest: None,
        computed_file_digest: None,
        file_hash: HashStatus::NotChecked,
        file_hash_note: Some(note.to_string()),
        signature_check: Verified::Unsupported { reason: note.to_string() },
        chain: None,
        unsigned_attributes: Vec::new(),
        notes: Vec::new(),
    }
}

/// The digest named inside `SpcIndirectDataContent`: `(algorithm OID, value)`. `econtent` is that
/// SEQUENCE's content: an attribute, then a DigestInfo.
fn indirect_digest(econtent: &[u8]) -> Option<(String, Vec<u8>)> {
    let attribute = der::read(econtent, 0)?;
    let info = der::read(econtent, attribute.end())?;
    if info.tag != 0x30 {
        return None;
    }
    let kids = der::children(econtent, &info);
    let alg = kids.first().filter(|t| t.tag == 0x30)?;
    let oid = der::seq_alg_oid(econtent, alg)?;
    let value = kids.get(1).filter(|t| t.tag == 0x04)?.bytes(econtent)?.to_vec();
    Some((oid, value))
}

/// The signer infos of a `SignedData`: how many, and the first one's unsigned attribute types.
fn signer_facts(cms: &[u8]) -> (usize, Vec<String>) {
    let walk = || -> Option<(usize, Vec<String>)> {
        let ci = der::read(cms, 0)?;
        let top = der::children(cms, &ci);
        let explicit = top.get(1).filter(|t| t.tag == 0xa0)?;
        let sd = der::children(cms, explicit).into_iter().next()?;
        let infos = der::children(cms, &sd).into_iter().rfind(|t| t.tag == 0x31)?;
        let signers = der::children(cms, &infos);
        let mut attrs = Vec::new();
        if let Some(first) = signers.first()
            && let Some(unsigned) = der::children(cms, first).into_iter().find(|t| t.tag == 0xa1)
        {
            for a in der::children(cms, &unsigned) {
                let Some(oid) = der::children(cms, &a).first().filter(|t| t.tag == 0x06).and_then(|t| t.bytes(cms)).map(der::oid) else { continue };
                attrs.push(match oid.as_str() {
                    "1.2.840.113549.1.9.6" => "countersignature (timestamp)".to_string(),
                    "1.3.6.1.4.1.311.3.3.1" => "RFC 3161 timestamp".to_string(),
                    "1.3.6.1.4.1.311.2.4.1" => "nested signature".to_string(),
                    _ => oid,
                });
            }
        }
        Some((signers.len(), attrs))
    };
    walk().unwrap_or((0, Vec::new()))
}

/// The parts of the file an Authenticode hash covers, in the order Windows hashes them (PE/COFF
/// specification, "Authenticode PE Image Hash"): the headers, each section in the order its data
/// sits in the file, then the data after them — all but the header checksum and the certificate
/// table. A gap between sections isn't hashed. The bytes after the sections are counted from the
/// headers and the sections' sizes, as Windows counts them, so a count that ends before the last
/// section's data hashes that data again.
fn hashed_ranges<'a>(data: &'a [u8], h: &PeHeaders) -> Result<Vec<&'a [u8]>, String> {
    let (cert_off, cert_size) = h.dir(SECURITY_DIR).ok_or("there is no certificate table")?;
    let opt = h.pe_offset.checked_add(24).ok_or("the headers are out of range")?;
    let checksum = opt.checked_add(64).ok_or("the headers are out of range")?;
    let dirs = opt.checked_add(if h.pe32_plus { 112 } else { 96 }).ok_or("the headers are out of range")?;
    let entry = dirs.checked_add(SECURITY_DIR * 8).ok_or("the headers are out of range")?;
    let header_end = usize::try_from(h.size_of_headers).map_err(|_| "the headers are out of range")?;
    let cert_start = usize::try_from(cert_off).map_err(|_| "the table is out of range")?;
    let cert_end = cert_start.checked_add(usize::try_from(cert_size).map_err(|_| "the table is out of range")?).ok_or("the table is out of range")?;
    if cert_end > data.len() {
        return Err("the certificate table runs past the end of the file".into());
    }
    let ordered = checksum.checked_add(4).is_some_and(|c| c <= entry) && entry.checked_add(8).is_some_and(|e| e <= header_end);
    if !ordered || header_end > cert_start {
        return Err("the certificate table overlaps the headers".into());
    }
    // The section table is read up to 96 entries and up to the end of the file. A file that
    // declares more would leave sections out of the hash, so it isn't hashed.
    let declared = h.pe_offset.checked_add(6).and_then(|o| read_u16(data, o)).map_or(0, usize::from);
    if declared > h.sections.len() {
        return Err(format!("the file declares {declared} sections, but only {} were read", h.sections.len()));
    }
    let part = |from: usize, to: usize| data.get(from..to).ok_or_else(|| "the file is too short for its headers".to_string());
    let mut ranges = vec![part(0, checksum)?, part(checksum + 4, entry)?, part(entry + 8, header_end)?];
    let mut sections: Vec<&crate::pe::PeSection> = h.sections.iter().filter(|s| s.raw_size > 0).collect();
    sections.sort_by_key(|s| s.raw_offset);
    // The running count of hashed bytes: the headers, then every section's data.
    let mut hashed = header_end;
    for s in sections {
        let from = usize::try_from(s.raw_offset).map_err(|_| "a section is out of range")?;
        let len = usize::try_from(s.raw_size).map_err(|_| "a section is out of range")?;
        let to = from.checked_add(len).ok_or("a section is out of range")?;
        ranges.push(data.get(from..to).ok_or_else(|| "a section runs past the end of the file".to_string())?);
        hashed = hashed.checked_add(len).ok_or("the sections are too large")?;
    }
    // Everything after them: Windows takes the whole file, less the sections' sizes and the
    // certificate table's, from the running count. A gap between sections isn't hashed, and a
    // count that ends before the last section's data hashes that data again.
    let cert_len = usize::try_from(cert_size).map_err(|_| "the table is out of range")?;
    let extra_len = data.len().checked_sub(hashed).and_then(|left| left.checked_sub(cert_len)).ok_or("the sections cover the certificate table")?;
    let extra_end = hashed.checked_add(extra_len).ok_or("the sections are too large")?;
    ranges.push(data.get(hashed..extra_end).ok_or_else(|| "the sections run past the end of the file".to_string())?);
    Ok(ranges)
}

/// Compute the Authenticode hash of a PE file with `digest`.
pub fn file_hash(data: &[u8], h: &PeHeaders, digest: Digest) -> Result<Vec<u8>, String> {
    let ranges = hashed_ranges(data, h)?;
    Ok(verify::hash_parts(digest, &ranges))
}

fn read_signature(data: &[u8], h: &PeHeaders, payload: &[u8], table_ok: bool) -> SignatureReport {
    let Some(sd) = pkcs7::parse_signed_data(payload) else {
        return unparsed("the block isn't well-formed PKCS#7 SignedData");
    };
    let (signer_count, unsigned_attributes) = signer_facts(payload);
    let certificates: Vec<CertInfo> = sd.certificates.iter().filter_map(|c| der::certificate(c)).collect();
    let signer = sd.signer_cert().and_then(der::certificate);
    let mut notes = Vec::new();
    if sd.non_minimal_length {
        notes.push(der::NON_MINIMAL_LENGTH_NOTE.into());
    }
    if signer_count > 1 {
        notes.push(format!("The block has {signer_count} signers; only the first was read."));
    }
    if sd.content_type.as_deref().is_some_and(|c| c != SPC_INDIRECT_DATA) {
        notes.push("The signed content isn't marked as Authenticode data.".to_string());
    }

    let econtent = sd.econtent.as_deref();
    let stated = econtent.and_then(indirect_digest);
    let mut report = SignatureReport {
        parsed: true,
        signer,
        signer_count,
        certificates,
        digest_algorithm: sd.digest.map(digest_name),
        signature_algorithm: Some(named_oid(&sd.signature_algorithm)),
        file_digest_algorithm: stated.as_ref().map(|(o, _)| pkcs7::digest_from_oid(o).map_or_else(|| named_oid(o), digest_name)),
        file_digest: stated.as_ref().map(|(_, v)| hexs(v)),
        computed_file_digest: None,
        file_hash: HashStatus::NotChecked,
        file_hash_note: None,
        signature_check: Verified::Unsupported { reason: "the block holds no signed content".into() },
        chain: None,
        unsigned_attributes,
        notes,
    };

    // The file's hash against the signed one.
    match &stated {
        None => report.file_hash_note = Some("the signed content names no file hash".into()),
        Some(_) if !table_ok => report.file_hash_note = Some("the certificate table runs past the end of the file".into()),
        Some((oid, value)) => match pkcs7::digest_from_oid(oid) {
            None => report.file_hash_note = Some(format!("hash algorithm {} isn't one this reader computes", named_oid(oid))),
            Some(d) => match file_hash(data, h, d) {
                Err(why) => report.file_hash_note = Some(why),
                Ok(computed) => {
                    report.file_hash = if computed == *value { HashStatus::Matches } else { HashStatus::Differs };
                    report.computed_file_digest = Some(hexs(&computed));
                }
            },
        },
    }

    // The signature over the signed content, and how the embedded certificates link.
    if let Some(content) = econtent
        && let Some(check) = verify::verify_pkcs7_detached(content, payload)
    {
        report.signature_check = check.signature_valid;
        report.chain = Some(check.chain);
        for note in check.notes {
            if !report.notes.contains(&note) {
                report.notes.push(note);
            }
        }
    }
    report
}
