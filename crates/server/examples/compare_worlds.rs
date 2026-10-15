//! Compares the blocks of two worlds in a box, such as a vanilla world and
//! a Ferrocraft world after the same commands:
//!
//!   cargo run --release --example compare_worlds -- <world a> <world b> x0 y0 z0 x1 y1 z1
//!
//! Prints every block that differs, `-` where a world has no saved chunk
//! (Ferrocraft does not save untouched superflat chunks); `ALL=1` prints
//! every block.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use ferrocraft::data::GameData;
use ferrocraft::storage::{Storage, StoredChunk};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [a, b, rest @ ..] = &args[..] else {
        anyhow::bail!("usage: compare_worlds <world a> <world b> x0 y0 z0 x1 y1 z1");
    };
    let corners: Vec<i32> = rest
        .iter()
        .map(|v| v.parse().context("coordinates"))
        .collect::<anyhow::Result<_>>()?;
    let [x0, y0, z0, x1, y1, z1] = corners[..] else {
        anyhow::bail!("six coordinates");
    };
    let data = GameData::load()?;
    let load = |dir: &str| -> anyhow::Result<HashMap<(i32, i32), StoredChunk>> {
        let chunks = Storage::open(Path::new(dir))?.load_chunks(&data)?;
        Ok(chunks.into_iter().map(|c| (c.pos, c)).collect())
    };
    let (a, b) = (load(a)?, load(b)?);
    let name = |state: Option<u32>| match state.and_then(|s| Some((s, data.block_of(s)?.1))) {
        Some((state, block)) => {
            let properties: Vec<String> = block
                .values(state)
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            format!("{}[{}]", block.name, properties.join(","))
        }
        None => "-".into(),
    };
    let block = |world: &HashMap<(i32, i32), StoredChunk>, x: i32, y: i32, z: i32| {
        let chunk = world.get(&(x >> 4, z >> 4))?;
        let y = usize::try_from(y - ferrocraft::world::MIN_Y).ok()?;
        let index = (y % 16) * 256 + (z & 15) as usize * 16 + (x & 15) as usize;
        Some(chunk.states.get(y / 16)?[index])
    };
    let all = std::env::var_os("ALL").is_some();
    let (mut same, mut different) = (0, 0);
    for y in y0..=y1 {
        for z in z0..=z1 {
            for x in x0..=x1 {
                let (sa, sb) = (block(&a, x, y, z), block(&b, x, y, z));
                if sa == sb {
                    same += 1;
                } else {
                    different += 1;
                }
                if all || sa != sb {
                    println!("{x} {y} {z}: {} vs {}", name(sa), name(sb));
                }
            }
        }
    }
    println!("same {same}, different {different}");
    Ok(())
}
