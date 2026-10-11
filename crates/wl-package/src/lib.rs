//! Read-only identification of Switch content packages and extracted executable inputs.
//!
//! Every read is bounded. Package payloads are streamed only for optional content
//! verification. No decryption is performed and no key material is read or emitted:
//! ticket title-key blocks and NCA key areas are never copied out of the input.
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::Path,
};
use wl_common::Result;
use wl_nso::Module;

const MAX_PFS0_ENTRIES: u32 = 4096;
const MAX_PFS0_STRINGS: u32 = 1024 * 1024;
const MAX_XML: u64 = 1024 * 1024;
const MAX_NPDM: u64 = 1024 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 4096;
pub const EXEFS_MODULES: [&str; 13] = [
    "rtld", "main", "subsdk0", "subsdk1", "subsdk2", "subsdk3", "subsdk4", "subsdk5", "subsdk6",
    "subsdk7", "subsdk8", "subsdk9", "sdk",
];

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("bounded slice"))
}
fn le64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("bounded slice"))
}
fn read_at(file: &mut File, offset: u64, size: usize) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;
    let mut buffer = vec![0; size];
    file.read_exact(&mut buffer)
        .map_err(|e| format!("read {size:#x} bytes at {offset:#x}: {e}"))?;
    Ok(buffer)
}
fn sha256_range(file: &mut File, offset: u64, size: u64) -> Result<String> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 8 * 1024 * 1024];
    let mut left = size;
    while left > 0 {
        let n = left.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..n])
            .map_err(|e| e.to_string())?;
        hasher.update(&buffer[..n]);
        left -= n as u64;
    }
    Ok(hex(&hasher.finalize()))
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub path: String,
    pub items: Vec<Item>,
    pub relationships: Vec<Relationship>,
}
#[derive(Debug, Serialize)]
pub struct Item {
    pub path: String,
    pub size: u64,
    #[serde(flatten)]
    pub content: Content,
}
#[derive(Debug, Serialize)]
#[serde(tag = "format", rename_all = "kebab-case")]
pub enum Content {
    Pfs0 {
        /// Bracketed identifiers in the file name. Unverified: names are not metadata.
        filename_claims: Vec<String>,
        entries: Vec<PfsEntry>,
    },
    Nca(Nca),
    Nso(NsoIdentity),
    Npdm(Npdm),
    Exefs {
        modules: Vec<NsoIdentity>,
        npdm: Option<Npdm>,
    },
    Xci {
        note: &'static str,
    },
    Unrecognized {
        signature: String,
    },
}
#[derive(Debug, Serialize)]
pub struct PfsEntry {
    pub name: String,
    pub offset: u64,
    pub size: u64,
    pub detail: EntryDetail,
}
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum EntryDetail {
    Nca(Nca),
    ContentMetaXml(ContentMeta),
    Ticket(Ticket),
    CertificateChain,
    Other,
}
#[derive(Debug, Serialize)]
pub struct Nca {
    /// `true` when offset 0x200 holds a plaintext NCA0/NCA2/NCA3 magic.
    pub header_plaintext: bool,
    pub header: Option<NcaHeader>,
    /// For package entries: the 32-hex-digit content ID taken from the entry name.
    pub content_id: Option<String>,
    /// Present only with `--verify-content`.
    pub sha256: Option<String>,
    /// Content IDs are the first 16 bytes of the content's SHA-256.
    pub content_id_verified: Option<bool>,
}
#[derive(Debug, Serialize)]
pub struct NcaHeader {
    pub magic: String,
    pub distribution: &'static str,
    pub content_type: &'static str,
    pub key_generation: u8,
    pub content_size: u64,
    pub program_id: String,
    pub content_index: u32,
    pub sdk_addon_version: String,
    pub uses_rights_id: bool,
    pub sections: Vec<(u64, u64)>,
}
#[derive(Debug, Default, Serialize)]
pub struct ContentMeta {
    pub meta_type: String,
    pub id: String,
    pub version: u64,
    pub required_system_version: Option<u64>,
    pub patch_id: Option<String>,
    pub key_generation_min: Option<u64>,
    pub contents: Vec<ContentRecord>,
}
#[derive(Debug, Serialize)]
pub struct ContentRecord {
    pub content_type: String,
    pub id: String,
    pub size: u64,
    pub sha256: String,
    pub key_generation: Option<u64>,
}
#[derive(Debug, Serialize)]
pub struct Ticket {
    pub signature_type: String,
    pub issuer: String,
    pub title_key_type: &'static str,
    pub key_generation: u8,
    pub rights_id: String,
    pub title_id: String,
    pub bound_to_device_or_account: bool,
}
#[derive(Debug, Serialize)]
pub struct NsoIdentity {
    pub name: String,
    pub sha256: String,
    pub module_id: String,
    pub text_size: u32,
    pub rodata_size: u32,
    pub data_size: u32,
    pub bss_size: u32,
    pub mod0: bool,
    pub relocations: usize,
    pub symbols: usize,
}
#[derive(Debug, Serialize)]
pub struct Npdm {
    pub name: String,
    pub is_64_bit: bool,
    pub address_space: &'static str,
    pub main_thread_priority: u8,
    pub main_thread_core: u8,
    pub main_thread_stack_size: u32,
    pub system_resource_size: u32,
    pub version: u32,
    pub program_id: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct Relationship {
    pub kind: &'static str,
    pub subject: String,
    pub object: String,
    pub evidence: String,
}

fn classify_nca(head: &[u8]) -> Nca {
    let magic = &head[0x200..0x204];
    let plaintext = [b"NCA0", b"NCA2", b"NCA3"].iter().any(|m| magic == *m);
    let header = plaintext.then(|| NcaHeader {
        magic: String::from_utf8_lossy(magic).into_owned(),
        distribution: if head[0x204] == 0 {
            "download"
        } else {
            "game-card"
        },
        content_type: match head[0x205] {
            0 => "program",
            1 => "meta",
            2 => "control",
            3 => "manual",
            4 => "data",
            5 => "public-data",
            _ => "unknown",
        },
        key_generation: head[0x206].max(head[0x220]),
        content_size: le64(head, 0x208),
        program_id: format!("{:016x}", le64(head, 0x210)),
        content_index: le32(head, 0x218),
        sdk_addon_version: {
            let v = le32(head, 0x21c);
            format!("{}.{}.{}", v >> 24, (v >> 16) & 0xff, (v >> 8) & 0xff)
        },
        uses_rights_id: head[0x230..0x240].iter().any(|b| *b != 0),
        sections: (0..4)
            .map(|i| {
                let at = 0x240 + i * 0x10;
                (
                    u64::from(le32(head, at)) * 0x200,
                    u64::from(le32(head, at + 4)) * 0x200,
                )
            })
            .filter(|(start, end)| end > start)
            .collect(),
    });
    Nca {
        header_plaintext: plaintext,
        header,
        content_id: None,
        sha256: None,
        content_id_verified: None,
    }
}

fn xml_values<'a>(text: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut values = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        values.push(after[..end].trim());
        rest = &after[end + close.len()..];
    }
    values
}
fn xml_number(value: &str) -> Result<u64> {
    match value.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16),
        None => value.parse(),
    }
    .map_err(|e| format!("CNMT XML number {value:?}: {e}"))
}
/// Parses the fixed CNMT XML vocabulary written by packaging tools.
pub fn parse_cnmt_xml(text: &str) -> Result<ContentMeta> {
    let body_end = text.find("<Content>").unwrap_or(text.len());
    let tail_start = text
        .rfind("</Content>")
        .map_or(0, |i| i + "</Content>".len());
    let header = format!("{}{}", &text[..body_end], &text[tail_start..]);
    let one = |tag: &str| xml_values(&header, tag).first().map(|v| v.to_string());
    let mut meta = ContentMeta {
        meta_type: one("Type").ok_or("CNMT XML lacks <Type>")?,
        id: one("Id").ok_or("CNMT XML lacks <Id>")?,
        version: xml_number(&one("Version").ok_or("CNMT XML lacks <Version>")?)?,
        required_system_version: one("RequiredSystemVersion")
            .map(|v| xml_number(&v))
            .transpose()?,
        patch_id: one("PatchId"),
        key_generation_min: one("KeyGenerationMin")
            .map(|v| xml_number(&v))
            .transpose()?,
        contents: Vec::new(),
    };
    for block in xml_values(text, "Content") {
        let field = |tag: &str| {
            xml_values(block, tag)
                .first()
                .map(|v| v.to_string())
                .ok_or_else(|| format!("CNMT <Content> lacks <{tag}>"))
        };
        meta.contents.push(ContentRecord {
            content_type: field("Type")?,
            id: field("Id")?,
            size: xml_number(&field("Size")?)?,
            sha256: field("Hash")?,
            key_generation: field("KeyGeneration")
                .ok()
                .map(|v| xml_number(&v))
                .transpose()?,
        });
    }
    Ok(meta)
}
/// Ticket metadata. The title-key block (0x100 bytes after the issuer) is never read out.
pub fn parse_ticket(t: &[u8]) -> Result<Ticket> {
    if t.len() < 4 {
        return Err("ticket truncated".into());
    }
    let kind = le32(t, 0);
    let (signature, padding) = match kind {
        0x10000 | 0x10003 => (0x200, 0x3c),
        0x10001 | 0x10004 => (0x100, 0x3c),
        0x10002 | 0x10005 => (0x3c, 0x40),
        _ => return Err(format!("unknown ticket signature type {kind:#x}")),
    };
    let h = 4 + signature + padding;
    if t.len() < h + 0x180 {
        return Err("ticket body truncated".into());
    }
    let rights = &t[h + 0x160..h + 0x170];
    Ok(Ticket {
        signature_type: format!("{kind:#x}"),
        issuer: String::from_utf8_lossy(&t[h..h + 0x40])
            .trim_end_matches('\0')
            .into(),
        title_key_type: match t[h + 0x141] {
            0 => "common",
            1 => "personalized",
            _ => "unknown",
        },
        key_generation: t[h + 0x145],
        rights_id: hex(rights),
        title_id: hex(&rights[..8]),
        bound_to_device_or_account: le64(t, h + 0x158) != 0 || le32(t, h + 0x170) != 0,
    })
}
pub fn parse_npdm(b: &[u8]) -> Result<Npdm> {
    if b.len() < 0x80 || &b[..4] != b"META" {
        return Err("NPDM lacks META header".into());
    }
    let flags = b[0xc];
    let aci = le32(b, 0x70) as usize;
    let program_id = (aci.checked_add(0x18).is_some_and(|end| end <= b.len())
        && &b[aci..aci + 4] == b"ACI0")
        .then(|| format!("{:016x}", le64(b, aci + 0x10)));
    Ok(Npdm {
        name: String::from_utf8_lossy(&b[0x20..0x30])
            .trim_end_matches('\0')
            .into(),
        is_64_bit: flags & 1 != 0,
        address_space: match (flags >> 1) & 7 {
            0 => "32-bit",
            1 => "64-bit-36",
            2 => "32-bit-no-reserved",
            3 => "64-bit-39",
            4 => "64-bit-42",
            _ => "unknown",
        },
        main_thread_priority: b[0xe],
        main_thread_core: b[0xf],
        main_thread_stack_size: le32(b, 0x1c),
        system_resource_size: le32(b, 0x14),
        version: le32(b, 0x18),
        program_id,
    })
}
fn nso_identity(name: &str, path: &Path) -> Result<NsoIdentity> {
    let m = Module::parse(&wl_nso::read_input(path)?).map_err(|e| format!("{name}: {e}"))?;
    Ok(NsoIdentity {
        name: name.into(),
        sha256: m.sha256.clone(),
        module_id: m.module_id.clone(),
        text_size: m.segments[0].memory_size,
        rodata_size: m.segments[1].memory_size,
        data_size: m.segments[2].memory_size,
        bss_size: m.bss_size,
        mod0: m.mod0.is_some(),
        relocations: m.mod0.as_ref().map_or(0, |d| d.dynamic.relocations.len()),
        symbols: m.mod0.as_ref().map_or(0, |d| d.dynamic.symbols.len()),
    })
}
fn filename_claims(name: &str) -> Vec<String> {
    name.split('[')
        .skip(1)
        .filter_map(|s| s.split(']').next())
        .map(str::to_owned)
        .collect()
}
fn pfs0(file: &mut File, name: &str, size: u64, verify: bool) -> Result<Content> {
    let head = read_at(file, 0, 16)?;
    let count = le32(&head, 4);
    let strings = le32(&head, 8);
    if count > MAX_PFS0_ENTRIES || strings > MAX_PFS0_STRINGS {
        return Err(format!(
            "PFS0 header declares {count} entries / {strings} string bytes"
        ));
    }
    let table = read_at(file, 16, count as usize * 24 + strings as usize)?;
    let names = &table[count as usize * 24..];
    let data = 16 + u64::from(count) * 24 + u64::from(strings);
    let mut entries = Vec::new();
    for i in 0..count as usize {
        let e = &table[i * 24..i * 24 + 24];
        let (offset, length, name_at) = (le64(e, 0), le64(e, 8), le32(e, 16) as usize);
        let end = names
            .get(name_at..)
            .and_then(|s| s.iter().position(|b| *b == 0))
            .ok_or("PFS0 entry name is outside the string table")?;
        let entry_name = String::from_utf8_lossy(&names[name_at..name_at + end]).into_owned();
        let start = data.checked_add(offset).ok_or("PFS0 offset overflow")?;
        if start.checked_add(length).is_none_or(|e| e > size) {
            return Err(format!("PFS0 entry {entry_name:?} exceeds the file"));
        }
        let detail = if entry_name.ends_with(".nca") {
            if length < 0x400 {
                return Err(format!("{entry_name}: NCA shorter than its header"));
            }
            let mut nca = classify_nca(&read_at(file, start, 0x400)?);
            let stem = entry_name.split('.').next().unwrap_or_default();
            if stem.len() == 32 && stem.bytes().all(|b| b.is_ascii_hexdigit()) {
                nca.content_id = Some(stem.to_ascii_lowercase());
            }
            if verify {
                let digest = sha256_range(file, start, length)?;
                nca.content_id_verified = nca.content_id.as_ref().map(|id| digest.starts_with(id));
                nca.sha256 = Some(digest);
            }
            EntryDetail::Nca(nca)
        } else if entry_name.ends_with(".cnmt.xml") && length <= MAX_XML {
            let text = read_at(file, start, length as usize)?;
            EntryDetail::ContentMetaXml(parse_cnmt_xml(&String::from_utf8_lossy(&text))?)
        } else if entry_name.ends_with(".tik") && length <= 0x1000 {
            EntryDetail::Ticket(parse_ticket(&read_at(file, start, length as usize)?)?)
        } else if entry_name.ends_with(".cert") {
            EntryDetail::CertificateChain
        } else {
            EntryDetail::Other
        };
        entries.push(PfsEntry {
            name: entry_name,
            offset: start,
            size: length,
            detail,
        });
    }
    Ok(Content::Pfs0 {
        filename_claims: filename_claims(name),
        entries,
    })
}
fn classify_file(path: &Path, verify: bool) -> Result<Item> {
    let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let head = read_at(&mut file, 0, size.min(0x400) as usize)?;
    let content = if head.starts_with(b"PFS0") {
        pfs0(&mut file, &name, size, verify)?
    } else if head.starts_with(b"NSO0") {
        Content::Nso(nso_identity(&name, path)?)
    } else if head.starts_with(b"META") && size <= MAX_NPDM {
        Content::Npdm(parse_npdm(&fs::read(path).map_err(|e| e.to_string())?)?)
    } else if head.len() >= 0x104 && &head[0x100..0x104] == b"HEAD" {
        Content::Xci {
            note: "game-card image detected; HFS0 partition parsing is not implemented",
        }
    } else if head.len() == 0x400 && name.ends_with(".nca") {
        let mut nca = classify_nca(&head);
        if verify {
            nca.sha256 = Some(sha256_range(&mut file, 0, size)?);
        }
        Content::Nca(nca)
    } else {
        Content::Unrecognized {
            signature: hex(&head[..head.len().min(8)]),
        }
    };
    Ok(Item {
        path: path.display().to_string(),
        size,
        content,
    })
}
fn exefs(dir: &Path) -> Result<Option<Item>> {
    let mut modules = Vec::new();
    for name in EXEFS_MODULES {
        for candidate in [dir.join(name), dir.join(format!("{name}.nso"))] {
            if candidate.is_file() {
                modules.push(nso_identity(name, &candidate)?);
            }
        }
    }
    if modules.is_empty() {
        return Ok(None);
    }
    let npdm_path = dir.join("main.npdm");
    let npdm = if npdm_path.is_file() {
        Some(parse_npdm(
            &fs::read(&npdm_path).map_err(|e| e.to_string())?,
        )?)
    } else {
        None
    };
    Ok(Some(Item {
        path: dir.display().to_string(),
        size: 0,
        content: Content::Exefs { modules, npdm },
    }))
}
fn relationships(items: &[Item]) -> Vec<Relationship> {
    let mut result = Vec::new();
    let mut metas = Vec::new();
    let mut tickets = Vec::new();
    for item in items {
        if let Content::Pfs0 { entries, .. } = &item.content {
            for e in entries {
                match &e.detail {
                    EntryDetail::ContentMetaXml(m) => metas.push((item, m)),
                    EntryDetail::Ticket(t) => tickets.push((item, t)),
                    _ => {}
                }
            }
            // A package's CNMT lists its contents and their full hashes.
            for (_, meta) in metas.iter().filter(|(i, _)| std::ptr::eq(*i, item)) {
                for c in &meta.contents {
                    let present = entries.iter().find(|e| {
                        e.name == format!("{}.nca", c.id) || e.name == format!("{}.cnmt.nca", c.id)
                    });
                    let evidence = match present.map(|e| &e.detail) {
                        Some(EntryDetail::Nca(n)) => match &n.sha256 {
                            Some(h) if *h == c.sha256 && e_size(present) == c.size => {
                                "entry present; size and full SHA-256 equal the CNMT record"
                            }
                            Some(_) => {
                                "entry present; SHA-256 or size DIFFERS from the CNMT record"
                            }
                            None => "entry present; hash not verified (use --verify-content)",
                        },
                        _ => "entry missing from package",
                    };
                    result.push(Relationship {
                        kind: "cnmt-content",
                        subject: format!("{} {}", meta.meta_type, meta.id),
                        object: format!("{} {}", c.content_type, c.id),
                        evidence: evidence.into(),
                    });
                }
            }
        }
    }
    for (_, meta) in &metas {
        if let Some(patch) = &meta.patch_id {
            let patch = patch.trim_start_matches("0x").to_ascii_lowercase();
            for (item, t) in &tickets {
                if t.title_id == patch {
                    result.push(Relationship {
                        kind: "application-patch",
                        subject: format!("Application {}", meta.id),
                        object: item.path.clone(),
                        evidence: format!("application CNMT PatchId {patch} equals the title ID in ticket rights ID {}", t.rights_id),
                    });
                }
            }
        }
    }
    result
}
fn e_size(e: Option<&PfsEntry>) -> u64 {
    e.map_or(0, |e| e.size)
}
/// Identifies `path`, a file or a directory (scanned to depth 3, symbolic links not followed).
pub fn intake(path: &Path, verify: bool) -> Result<Report> {
    let mut items = Vec::new();
    let mut pending = vec![(path.to_path_buf(), 0)];
    let mut visited = 0;
    while let Some((p, depth)) = pending.pop() {
        let meta = fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        if meta.is_dir() {
            if let Some(item) = exefs(&p)? {
                items.push(item);
            }
            if depth >= 3 {
                continue;
            }
            let mut children: Vec<_> = fs::read_dir(&p)
                .map_err(|e| e.to_string())?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .collect();
            visited += children.len();
            if visited > MAX_DIRECTORY_ENTRIES {
                return Err("directory scan exceeds 4096 entries".into());
            }
            children.sort();
            pending.extend(children.into_iter().rev().map(|c| (c, depth + 1)));
        } else if meta.is_file() {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let in_exefs = p.parent().is_some_and(|d| {
                EXEFS_MODULES
                    .iter()
                    .any(|m| name == *m || name == format!("{m}.nso"))
                    && d.join("main.npdm").is_file()
                    || name == "main.npdm"
                        && EXEFS_MODULES
                            .iter()
                            .any(|m| d.join(m).is_file() || d.join(format!("{m}.nso")).is_file())
            });
            if !in_exefs {
                items.push(classify_file(&p, verify)?);
            }
        }
    }
    let relationships = relationships(&items);
    Ok(Report {
        path: path.display().to_string(),
        items,
        relationships,
    })
}
