use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};
use wl_package::{Content, EntryDetail, intake, parse_cnmt_xml, parse_npdm, parse_ticket};

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let p =
            std::env::temp_dir().join(format!("wanderlust-intake-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
const CANARY: u8 = 0xa5;
fn ticket(title: &str) -> Vec<u8> {
    let mut t = vec![0u8; 0x2c0];
    t[..4].copy_from_slice(&0x10004u32.to_le_bytes());
    let h = 0x140;
    t[h..h + 9].copy_from_slice(b"Root-Test");
    t[h + 0x40..h + 0x140].fill(CANARY); // title-key block: must never be reported
    t[h + 0x145] = 16;
    let id = u64::from_str_radix(title, 16).unwrap().to_be_bytes();
    t[h + 0x160..h + 0x168].copy_from_slice(&id);
    t[h + 0x16f] = 16;
    t
}
fn nca(plaintext: bool) -> Vec<u8> {
    let mut n: Vec<u8> = (0..0x600u32).map(|i| (i * 7 + 3) as u8).collect();
    if plaintext {
        n[0x200..0x204].copy_from_slice(b"NCA3");
        n[0x205] = 0;
        n[0x210..0x218].copy_from_slice(&0x0100_0000_0000_1000u64.to_le_bytes());
        n[0x220] = 0x11;
        n[0x230..0x240].fill(0);
        n[0x240..0x250].fill(0);
        n[0x240..0x244].copy_from_slice(&2u32.to_le_bytes());
        n[0x244..0x248].copy_from_slice(&3u32.to_le_bytes());
        n[0x250..0x280].fill(0);
    }
    n
}
fn pfs0(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut names = Vec::new();
    let mut table = Vec::new();
    let mut offset = 0u64;
    for (name, data) in entries {
        table.extend(offset.to_le_bytes());
        table.extend((data.len() as u64).to_le_bytes());
        table.extend((names.len() as u32).to_le_bytes());
        table.extend(0u32.to_le_bytes());
        names.extend(name.as_bytes());
        names.push(0);
        offset += data.len() as u64;
    }
    let mut out = b"PFS0".to_vec();
    out.extend((entries.len() as u32).to_le_bytes());
    out.extend((names.len() as u32).to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(table);
    out.extend(names);
    for (_, data) in entries {
        out.extend(data);
    }
    out
}
fn content_id(data: &[u8]) -> String {
    Sha256::digest(data)[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn identifies_package_relationships_without_reporting_keys() {
    let t = Temp::new("pkg");
    let encrypted = nca(false);
    let plain = nca(true);
    let (eid, pid) = (content_id(&encrypted), content_id(&plain));
    let full = |d: &[u8]| -> String {
        Sha256::digest(d)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };
    let xml = format!(
        "<?xml version=\"1.0\"?><ContentMeta><Type>Application</Type><Id>0x0100000000001000</Id><Version>0</Version>\
         <Content><Type>Program</Type><Id>{eid}</Id><Size>{}</Size><Hash>{}</Hash><KeyGeneration>15</KeyGeneration></Content>\
         <Content><Type>Control</Type><Id>{pid}</Id><Size>{}</Size><Hash>{}</Hash><KeyGeneration>15</KeyGeneration></Content>\
         <RequiredSystemVersion>1006632960</RequiredSystemVersion><PatchId>0x0100000000001800</PatchId></ContentMeta>",
        encrypted.len(),
        full(&encrypted),
        plain.len(),
        "00".repeat(32)
    );
    fs::write(
        t.0.join("base [0100000000001000][v0].nsp"),
        pfs0(&[
            ("meta.cnmt.xml", xml.into_bytes()),
            (&format!("{eid}.nca"), encrypted),
            (&format!("{pid}.nca"), plain),
        ]),
    )
    .unwrap();
    fs::write(
        t.0.join("update.nsp"),
        pfs0(&[("x.tik", ticket("0100000000001800"))]),
    )
    .unwrap();
    let report = intake(&t.0, true).unwrap();
    let text = serde_json::to_string(&report).unwrap();
    assert!(
        !text.contains(&"a5".repeat(8)) && !text.contains("165,165"),
        "title-key bytes leaked"
    );
    let Content::Pfs0 {
        filename_claims,
        entries,
    } = &report.items[0].content
    else {
        panic!()
    };
    assert_eq!(filename_claims, &["0100000000001000", "v0"]);
    let EntryDetail::Nca(n) = &entries[1].detail else {
        panic!()
    };
    assert!(!n.header_plaintext && n.header.is_none());
    assert_eq!(n.content_id_verified, Some(true));
    let EntryDetail::Nca(n) = &entries[2].detail else {
        panic!()
    };
    let header = n.header.as_ref().unwrap();
    assert_eq!(
        (header.content_type, header.program_id.as_str()),
        ("program", "0100000000001000")
    );
    assert_eq!(header.sections, [(0x400, 0x600)]);
    let evidence: Vec<_> = report
        .relationships
        .iter()
        .map(|r| (r.kind, r.evidence.as_str()))
        .collect();
    assert!(evidence[0].1.contains("full SHA-256 equal"));
    assert!(evidence[1].1.contains("DIFFERS"));
    assert_eq!(evidence[2].0, "application-patch");
}
#[test]
fn parses_metadata_records() {
    let t = parse_ticket(&ticket("0100f2c0115b6800")).unwrap();
    assert_eq!((t.title_key_type, t.key_generation), ("common", 16));
    assert_eq!(t.rights_id, "0100f2c0115b68000000000000000010");
    assert!(!t.bound_to_device_or_account);
    assert!(parse_ticket(&[0, 0, 0, 0]).is_err());
    assert!(parse_cnmt_xml("<ContentMeta></ContentMeta>").is_err());
    let mut npdm = vec![0u8; 0x100];
    npdm[..4].copy_from_slice(b"META");
    npdm[0xc] = 1 | 3 << 1;
    npdm[0xe] = 44;
    npdm[0x1c..0x20].copy_from_slice(&0x100000u32.to_le_bytes());
    npdm[0x20..0x2b].copy_from_slice(b"Application");
    npdm[0x70..0x74].copy_from_slice(&0x80u32.to_le_bytes());
    npdm[0x80..0x84].copy_from_slice(b"ACI0");
    npdm[0x90..0x98].copy_from_slice(&0x0100_0000_0000_1000u64.to_le_bytes());
    let n = parse_npdm(&npdm).unwrap();
    assert!(n.is_64_bit);
    assert_eq!(n.address_space, "64-bit-39");
    assert_eq!(
        (n.main_thread_priority, n.main_thread_stack_size),
        (44, 0x100000)
    );
    assert_eq!(n.program_id.as_deref(), Some("0100000000001000"));
    assert!(parse_npdm(b"NOPE").is_err());
}
#[test]
fn rejects_truncated_and_oversized_packages() {
    let t = Temp::new("bad");
    let mut p = pfs0(&[("a.nca", nca(false))]);
    p.truncate(p.len() - 1);
    fs::write(t.0.join("short.nsp"), &p).unwrap();
    assert!(
        intake(&t.0.join("short.nsp"), false)
            .unwrap_err()
            .contains("exceeds the file")
    );
    let mut p = pfs0(&[]);
    p[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    fs::write(t.0.join("huge.nsp"), &p).unwrap();
    assert!(
        intake(&t.0.join("huge.nsp"), false)
            .unwrap_err()
            .contains("declares")
    );
}
