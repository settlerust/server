use server::{
    model::Record,
    reducer::{replay, state_hash},
    storage::merge_records,
};
use std::io::{self, BufRead};
fn main() -> anyhow::Result<()> {
    let records = io::stdin()
        .lock()
        .lines()
        .filter_map(|line| match line {
            Ok(line) if line.trim().is_empty() => None,
            other => Some(other),
        })
        .map(|line| Ok(serde_json::from_str::<Record>(&line?)?))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let records = merge_records(records)?;
    let room = replay(&records)?;
    println!(
        "{}",
        serde_json::json!({"game_id":room.id,"version":room.version,"hash":state_hash(&room)?,"finished":room.summary().finished})
    );
    Ok(())
}
