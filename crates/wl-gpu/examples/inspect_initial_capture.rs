//! Offline literal packet inventory. This does not execute MME, shaders or draws.
use std::collections::{BTreeMap, HashMap};
use wl_gpu::{fifo::Decoder, initial_capture::CommandSnapshot};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut paths: Vec<_> = std::env::args_os().skip(1).collect();
    paths.sort();
    if paths.is_empty() {
        return Err("usage: inspect_initial_capture FILE.gpfcap ...".into());
    }
    let mut channels = HashMap::new();
    let mut inventory = BTreeMap::new();
    for path in paths {
        let snapshot = CommandSnapshot::read(&path)?;
        let id = snapshot.identity();
        let (decoder, bindings, last_sequence) = channels
            .entry((id.process, id.channel))
            .or_insert((Decoder::default(), [0u32; 8], 0));
        if *last_sequence != 0 && id.sequence != *last_sequence + 1 {
            return Err("snapshot sequence gap; packet continuation unknown".into());
        }
        *last_sequence = id.sequence;
        eprintln!("snapshot {id:?}; pre-execution bytes, not a consumed-command trace");
        for (_, bytes) in snapshot.entries() {
            for raw in bytes.chunks_exact(4) {
                if let Some(write) = decoder.push_word(u32::from_le_bytes(raw.try_into()?))? {
                    if write.method == 0 {
                        bindings[write.subchannel as usize] = write.value;
                    }
                    let key = (
                        bindings[write.subchannel as usize],
                        write.subchannel,
                        write.method,
                    );
                    let (count, samples) = inventory.entry(key).or_insert((0u64, Vec::new()));
                    *count += 1;
                    if samples.len() < 4 && !samples.contains(&write.value) {
                        samples.push(write.value);
                    }
                }
            }
        }
    }
    println!("class subchannel byte-method writes sample-wire-values");
    for ((class, subchannel, method), (count, samples)) in inventory {
        println!("{class:04x} {subchannel} {method:04x} {count} {samples:08x?}");
    }
    for ((process, channel), (decoder, _, _)) in channels {
        if let Err(error) = decoder.finish() {
            eprintln!("process {process} channel {channel}: incomplete final snapshot: {error}");
        }
    }
    Ok(())
}
