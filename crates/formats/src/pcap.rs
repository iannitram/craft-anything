//! Shared saved-capture and protocol inspection. File admission and evidence policy remain local.
pub use craft_pcap::{
    Capture, Count, Decoded, Dns, DnsAnswer, DnsQuestion, Flow, FlowKey, FlowTable, Frame, Http, Interface, IpInfo, MAX_FLOWS, MAX_INVENTORY, MAX_PACKETS,
    Summary, TlsHello, Transport, decode, endpoint, ip_protocol_name, is_capture, iso8601, link_name, qtype_name, strip_userinfo,
};

impl From<craft_pcap::PcapError> for crate::FormatError {
    fn from(error: craft_pcap::PcapError) -> Self {
        match error {
            craft_pcap::PcapError::Truncated(reason) => Self::Truncated(reason),
            craft_pcap::PcapError::Malformed(reason) => Self::Malformed(reason),
            craft_pcap::PcapError::Unsupported(reason) => Self::Unsupported(reason),
        }
    }
}

pub fn summarize(data: &[u8], cancelled: &dyn Fn() -> bool) -> Result<Summary, crate::FormatError> {
    craft_pcap::summarize(data, cancelled).map_err(Into::into)
}

#[cfg(test)]
#[path = "pcap_legacy.rs"]
mod legacy;

#[cfg(test)]
mod parity_tests {
    use super::*;
    #[test]
    fn frames_protocols_summaries_and_errors_match() {
        let frames = craft_anything_testkit::pcap::sample_frames();
        for data in [craft_anything_testkit::pcap::pcap(&frames), craft_anything_testkit::pcap::pcapng(&frames), vec![], b"not a capture".to_vec()] {
            // Keep the retained admission oracle exercised, including truncated magic bytes.
            for end in 0..=data.len() {
                assert_eq!(legacy::is_capture(&data[..end]), is_capture(&data[..end]));
            }
            assert_eq!(format!("{:?}", legacy::summarize(&data, &|| false)), format!("{:?}", summarize(&data, &|| false)));
            assert_eq!(format!("{:?}", legacy::summarize(&data, &|| true)), format!("{:?}", summarize(&data, &|| true)));
            match (legacy::Capture::new(&data), Capture::new(&data)) {
                (Ok(old), Ok(new)) => {
                    let a: Vec<_> = old.collect();
                    let b: Vec<_> = new.collect();
                    assert_eq!(format!("{a:?}"), format!("{b:?}"));
                }
                (Err(a), Err(b)) => assert_eq!(a, crate::FormatError::from(b)),
                _ => panic!("capture parity mismatch"),
            }
        }
        for frame in frames {
            assert_eq!(serde_json::to_value(legacy::decode(1, &frame)).unwrap(), serde_json::to_value(decode(1, &frame)).unwrap());
        }
    }

    #[test]
    fn truncated_captures_and_packets_match_the_retained_oracle() {
        let frames = craft_anything_testkit::pcap::sample_frames();
        for data in [craft_anything_testkit::pcap::pcap(&frames), craft_anything_testkit::pcap::pcapng(&frames)] {
            for end in 0..=data.len() {
                let prefix = &data[..end];
                assert_eq!(format!("{:?}", legacy::summarize(prefix, &|| false)), format!("{:?}", summarize(prefix, &|| false)), "capture prefix {end}");
            }
        }
        for frame in frames {
            for end in 0..=frame.len() {
                assert_eq!(
                    serde_json::to_value(legacy::decode(1, &frame[..end])).unwrap(),
                    serde_json::to_value(decode(1, &frame[..end])).unwrap(),
                    "packet prefix {end}"
                );
            }
        }
    }

    #[test]
    fn public_protocol_labels_and_error_adapters_preserve_contracts() {
        for link in [0, 1, 12, 14, 101, 108, 113, 228, 229, 276, 105, 127, u32::MAX] {
            assert_eq!(legacy::link_name(link), link_name(link));
        }
        for protocol in 0..=u8::MAX {
            assert_eq!(legacy::ip_protocol_name(protocol), ip_protocol_name(protocol));
        }
        for kind in [1, 2, 5, 6, 12, 15, 16, 28, 33, 35, 39, 44, 52, 256, 64, 65, 257, 255, u16::MAX] {
            assert_eq!(legacy::qtype_name(kind), qtype_name(kind));
        }
        for (source, expected) in [
            (craft_pcap::PcapError::Truncated("cut".into()), crate::FormatError::Truncated("cut".into())),
            (craft_pcap::PcapError::Malformed("bad".into()), crate::FormatError::Malformed("bad".into())),
            (craft_pcap::PcapError::Unsupported("unknown".into()), crate::FormatError::Unsupported("unknown".into())),
        ] {
            assert_eq!(crate::FormatError::from(source), expected);
        }
    }
}
