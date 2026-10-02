//! N-tuple lookup tables as a learned leaf: one number per (table, pattern), summed over the
//! active patterns of a position, in win-probability units. Tables are learned by self-play
//! temporal-difference training (`crates/arena/src/bin/train.rs`) and deployed unchanged as the
//! leaf of the tunable search through `SearchConfig::leaf`.
//!
//! Weights and the two temporal-coherence accumulators are f32 bit patterns in atomics with
//! relaxed ordering, so training threads update shared tables without locks; a lost update
//! between two threads is tolerated. Evaluation sums in fixed feature order, so a frozen table
//! returns the same value for the same position on every call.
pub mod features;
use features::Offsets;
pub use features::{Layout, MAX_ACTIVE};
use settlers_engine::State;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

const MAGIC: &[u8; 8] = b"CTNTUP01";

#[inline(always)]
fn load(cell: &AtomicU32) -> f32 {
    f32::from_bits(cell.load(Relaxed))
}
#[inline(always)]
fn store(cell: &AtomicU32, value: f32) {
    cell.store(value.to_bits(), Relaxed)
}
fn atomic_vec(len: usize, value: f32) -> Vec<AtomicU32> {
    let bits = value.to_bits();
    let mut out = Vec::with_capacity(len);
    out.resize_with(len, || AtomicU32::new(bits));
    out
}

#[derive(Default, Clone, Copy, Debug)]
pub struct UpdateStats {
    pub updates: u64,
    pub abs_delta_sum: f64,
    pub beta_sum: f64,
    pub entry_updates: u64,
}

pub struct Model {
    pub layout: Layout,
    off: Offsets,
    weights: Vec<AtomicU32>,
    tc_e: Vec<AtomicU32>,
    tc_a: Vec<AtomicU32>,
}
impl Model {
    /// A fresh model valuing every position at `optimistic` (each entry holds its share).
    pub fn new(layout: Layout, optimistic: f32, trainable: bool) -> Self {
        let total = layout.total_entries();
        let init = optimistic / layout.active_count() as f32;
        Self {
            layout,
            off: Offsets::of(&layout),
            weights: atomic_vec(total, init),
            tc_e: if trainable {
                atomic_vec(total, 0.0)
            } else {
                Vec::new()
            },
            tc_a: if trainable {
                atomic_vec(total, 0.0)
            } else {
                Vec::new()
            },
        }
    }
    pub fn entries(&self) -> usize {
        self.weights.len()
    }
    pub fn trainable(&self) -> bool {
        !self.tc_a.is_empty()
    }
    pub fn bytes(&self) -> usize {
        (self.weights.len() + self.tc_e.len() + self.tc_a.len()) * 4
    }
    /// Active entries of a position for a seat.
    #[inline]
    pub fn features(&self, s: &State, seat: u8, out: &mut [u32; MAX_ACTIVE]) -> usize {
        features::features(&self.layout, &self.off, s, seat, out)
    }
    /// Sum of the active entries, in feature order.
    #[inline]
    pub fn value_of(&self, active: &[u32]) -> f32 {
        let mut sum = 0.0f32;
        for &index in active {
            sum += load(&self.weights[index as usize]);
        }
        sum
    }
    #[inline]
    pub fn value(&self, s: &State, seat: u8) -> f32 {
        let mut scratch = [0u32; MAX_ACTIVE];
        let n = self.features(s, seat, &mut scratch);
        self.value_of(&scratch[..n])
    }
    /// Temporal-coherence semi-gradient step toward `target`. Tables are shared across the
    /// board, so one entry can be active several times in a position; an entry used `k`
    /// times moves by `alpha * beta_i * delta * k / sum(k^2)`, once, and its accumulators take
    /// `k * delta`, so a fully coherent step moves the prediction by exactly `alpha * delta`.
    /// `beta_i = |E_i| / A_i` is the entry's coherence (1 before its first update), computed
    /// from the history before this error. `delta` is clamped to `±clamp`.
    #[inline]
    pub fn update(
        &self,
        active: &[u32],
        target: f32,
        alpha: f32,
        clamp: f32,
        stats: &mut UpdateStats,
    ) -> f32 {
        debug_assert!(self.trainable());
        let prediction = self.value_of(active);
        let delta = (target - prediction).clamp(-clamp, clamp);
        let abs_delta = delta.abs();
        let mut sorted = [0u32; MAX_ACTIVE];
        let n = active.len().min(MAX_ACTIVE);
        sorted[..n].copy_from_slice(&active[..n]);
        sorted[..n].sort_unstable();
        let mut norm = 0.0f32;
        let mut i = 0;
        while i < n {
            let mut j = i + 1;
            while j < n && sorted[j] == sorted[i] {
                j += 1;
            }
            let k = (j - i) as f32;
            norm += k * k;
            i = j;
        }
        let step = alpha * delta / norm.max(1.0);
        let mut i = 0;
        while i < n {
            let mut j = i + 1;
            while j < n && sorted[j] == sorted[i] {
                j += 1;
            }
            let k = (j - i) as f32;
            let index = sorted[i] as usize;
            let e = load(&self.tc_e[index]);
            let a = load(&self.tc_a[index]);
            let beta = if a > 0.0 { e.abs() / a } else { 1.0 };
            store(
                &self.weights[index],
                load(&self.weights[index]) + beta * step * k,
            );
            store(&self.tc_e[index], e + delta * k);
            store(&self.tc_a[index], a + abs_delta * k);
            stats.beta_sum += f64::from(beta);
            stats.entry_updates += 1;
            i = j;
        }
        stats.updates += 1;
        stats.abs_delta_sum += f64::from(abs_delta);
        delta
    }
    /// Entries that have received at least one update (a full scan).
    pub fn touched_entries(&self) -> u64 {
        self.tc_a.iter().filter(|a| load(a) > 0.0).count() as u64
    }
    /// A frozen copy of the current weights without accumulators.
    pub fn snapshot(&self) -> Self {
        let weights: Vec<AtomicU32> = self
            .weights
            .iter()
            .map(|w| AtomicU32::new(w.load(Relaxed)))
            .collect();
        Self {
            layout: self.layout,
            off: self.off,
            weights,
            tc_e: Vec::new(),
            tc_a: Vec::new(),
        }
    }
    /// FNV-1a over the weight bits: a content fingerprint for logs and labels.
    pub fn fingerprint(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for cell in &self.weights {
            hash ^= u64::from(cell.load(Relaxed));
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
    pub fn write_to<W: Write>(&self, out: &mut W, with_tc: bool) -> Result<(), String> {
        out.write_all(MAGIC).map_err(|e| e.to_string())?;
        let spec = self.layout.spec();
        out.write_all(&(spec.len() as u32).to_le_bytes())
            .map_err(|e| e.to_string())?;
        out.write_all(spec.as_bytes()).map_err(|e| e.to_string())?;
        out.write_all(&(self.weights.len() as u64).to_le_bytes())
            .map_err(|e| e.to_string())?;
        let tc = with_tc && self.trainable();
        out.write_all(&[u8::from(tc)]).map_err(|e| e.to_string())?;
        write_atomics(out, &self.weights)?;
        if tc {
            write_atomics(out, &self.tc_e)?;
            write_atomics(out, &self.tc_a)?;
        }
        out.flush().map_err(|e| e.to_string())
    }
    /// Write the model atomically. `with_tc` keeps the accumulators so training can resume;
    /// a frozen candidate is written without them.
    pub fn save(&self, path: &std::path::Path, with_tc: bool) -> Result<(), String> {
        let tmp = path.with_extension("tmp");
        {
            let file =
                std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
            let mut out = std::io::BufWriter::with_capacity(1 << 20, file);
            self.write_to(&mut out, with_tc)?;
        }
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }
    pub fn read_from<R: Read>(input: &mut R, trainable: bool) -> Result<Self, String> {
        let mut magic = [0u8; 8];
        input.read_exact(&mut magic).map_err(|e| e.to_string())?;
        if &magic != MAGIC {
            return Err("not a CTNTUP01 table file".into());
        }
        let mut len = [0u8; 4];
        input.read_exact(&mut len).map_err(|e| e.to_string())?;
        let mut spec = vec![0u8; u32::from_le_bytes(len) as usize];
        input.read_exact(&mut spec).map_err(|e| e.to_string())?;
        let layout = Layout::parse(std::str::from_utf8(&spec).map_err(|e| e.to_string())?)?;
        let mut count = [0u8; 8];
        input.read_exact(&mut count).map_err(|e| e.to_string())?;
        let count = u64::from_le_bytes(count) as usize;
        if count != layout.total_entries() {
            return Err(format!("entry count {count} does not match layout"));
        }
        let mut flag = [0u8; 1];
        input.read_exact(&mut flag).map_err(|e| e.to_string())?;
        let mut model = Self::new(layout, 0.0, trainable);
        read_atomics(input, &model.weights)?;
        if flag[0] == 1 && trainable {
            read_atomics(input, &model.tc_e)?;
            read_atomics(input, &model.tc_a)?;
        }
        if !trainable {
            model.tc_e = Vec::new();
            model.tc_a = Vec::new();
        }
        Ok(model)
    }
    pub fn load(path: &std::path::Path, trainable: bool) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut input = std::io::BufReader::with_capacity(1 << 20, file);
        Self::read_from(&mut input, trainable).map_err(|e| format!("{}: {e}", path.display()))
    }
}
fn write_atomics<W: Write>(out: &mut W, cells: &[AtomicU32]) -> Result<(), String> {
    let mut buffer = Vec::with_capacity(1 << 18);
    for chunk in cells.chunks(1 << 16) {
        buffer.clear();
        for cell in chunk {
            buffer.extend_from_slice(&cell.load(Relaxed).to_le_bytes());
        }
        out.write_all(&buffer).map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn read_atomics<R: Read>(input: &mut R, cells: &[AtomicU32]) -> Result<(), String> {
    let mut buffer = vec![0u8; 1 << 18];
    for chunk in cells.chunks(1 << 16) {
        let bytes = chunk.len() * 4;
        input
            .read_exact(&mut buffer[..bytes])
            .map_err(|e| e.to_string())?;
        for (cell, word) in chunk.iter().zip(buffer[..bytes].as_chunks::<4>().0) {
            cell.store(u32::from_le_bytes(*word), Relaxed);
        }
    }
    Ok(())
}

/// Frozen tables loaded once per process and shared by every seat that names the same path.
#[cfg(not(target_arch = "wasm32"))]
pub fn shared(path: &str) -> Result<std::sync::Arc<Model>, String> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Model>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache
        .lock()
        .map_err(|_| "table cache poisoned".to_string())?;
    if let Some(model) = guard.get(path) {
        return Ok(model.clone());
    }
    let model = Arc::new(Model::load(std::path::Path::new(path), false)?);
    guard.insert(path.to_string(), model.clone());
    Ok(model)
}

#[cfg(test)]
#[path = "../../tests/internal/ntuple.rs"]
mod tests;
