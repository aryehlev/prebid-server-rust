//! Test support: the generators, digests and script runner shared with `testdata/go`.
//!
//! Everything here mirrors the Go generator line for line, so that both sides draw the same
//! random operations and render their outcomes as the same text.

use std::{
    env,
    error::Error,
    fmt, fs,
    path::PathBuf,
    str,
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
};

use super::{Cache, CacheError, CacheIterator, Timer};

/// The result type of every test in this crate.
pub(super) type TestResult = Result<(), Box<dyn Error>>;

/// Fails the test with `what` unless `condition` holds.
pub(super) fn check(condition: bool, what: &str) -> TestResult {
    if condition { Ok(()) } else { Err(what.into()) }
}

/// Fails the test unless `got == want`.
pub(super) fn check_eq<G, W>(got: &G, want: &W, what: &str) -> TestResult
where
    G: PartialEq<W> + fmt::Debug + ?Sized,
    W: fmt::Debug + ?Sized,
{
    if got == want {
        Ok(())
    } else {
        Err(format!("{what}: got {got:?}, want {want:?}").into())
    }
}

/// Reads `testdata/golden/<name>`.
pub(super) fn read_golden(name: &str) -> Result<String, Box<dyn Error>> {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "testdata", "golden", name]
        .iter()
        .collect();
    Ok(fs::read_to_string(path)?)
}

/// The golden lines whose first field is `kind`, split into their other fields.
pub(super) fn golden_lines<'g>(golden: &'g str, kind: &str) -> Vec<Vec<&'g str>> {
    golden
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            (fields.next() == Some(kind)).then(|| fields.collect())
        })
        .collect()
}

/// Field `index` of a golden line.
pub(super) fn field<'g>(fields: &[&'g str], index: usize) -> Result<&'g str, Box<dyn Error>> {
    Ok(fields
        .get(index)
        .copied()
        .ok_or_else(|| format!("golden line {fields:?} has no field {index}"))?)
}

/// Decodes lower-case hex.
pub(super) fn hex_decode(text: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    text.as_bytes()
        .chunks(2)
        .map(|pair| Ok(u8::from_str_radix(str::from_utf8(pair)?, 16)?))
        .collect()
}

/// A fake clock the tests set and advance; Go's `fakeTimer`.
///
/// With a positive `tick_every` it also advances by itself, one second every `tick_every` calls
/// to [`Timer::now`], so that a cache reading its clock more or less often than Go's changes
/// the results.
#[derive(Debug, Default)]
pub(super) struct FakeTimer {
    now: AtomicU32,
    calls: AtomicU64,
    tick_every: u64,
}

impl FakeTimer {
    /// A shared clock at `now`.
    pub(super) fn at(now: u32) -> Arc<Self> {
        Self::ticking(now, 0)
    }

    /// A shared clock at `now` that advances every `tick_every` reads (never when `0`).
    pub(super) fn ticking(now: u32, tick_every: u64) -> Arc<Self> {
        Arc::new(Self {
            now: AtomicU32::new(now),
            calls: AtomicU64::new(0),
            tick_every,
        })
    }

    /// Moves the clock by `seconds`, wrapping like Go's `uint32`.
    pub(super) fn advance(&self, seconds: u32) {
        let _ = self.now.fetch_add(seconds, Ordering::SeqCst);
    }

    /// Moves the clock back by `seconds`, wrapping like Go's `uint32`.
    pub(super) fn rewind(&self, seconds: u32) {
        let _ = self.now.fetch_sub(seconds, Ordering::SeqCst);
    }

    /// The time, without counting as a read (Go's `clock.now` field).
    pub(super) fn current(&self) -> u32 {
        self.now.load(Ordering::SeqCst)
    }

    /// The number of [`Timer::now`] calls so far (Go's `clock.calls`).
    pub(super) fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Timer for FakeTimer {
    /// Go: `fakeTimer.Now`.
    fn now(&self) -> u32 {
        let calls = self.calls.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
        if self.tick_every > 0 && calls.is_multiple_of(self.tick_every) {
            let _ = self.now.fetch_add(1, Ordering::SeqCst);
        }
        self.current()
    }
}

/// A cache on a fake clock.
pub(super) type FakeCache = Cache<Arc<FakeTimer>>;

/// splitmix64, shared with the Go generator.
#[derive(Debug, Clone, Copy)]
pub(super) struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Go: `splitmix64{state: seed}`.
    pub(super) const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Go: `next`.
    pub(super) const fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Go: `below(n)`, `next() % n`, or `0` when `n` is `0`.
    pub(super) const fn below(&mut self, n: u64) -> u64 {
        let value = self.next_u64();
        match value.checked_rem(n) {
            Some(rem) => rem,
            None => 0,
        }
    }

    /// [`SplitMix64::below`] of a non-negative Go `int`, as a Go `int`.
    fn below_int(&mut self, n: i64) -> i64 {
        i64::try_from(self.below(n.cast_unsigned())).unwrap_or(i64::MAX)
    }

    /// Go: `fill`.
    pub(super) fn fill(&mut self, bytes: &mut [u8]) {
        for chunk in bytes.chunks_mut(8) {
            let word = self.next_u64().to_le_bytes();
            for (byte, value) in chunk.iter_mut().zip(word) {
                *byte = value;
            }
        }
    }
}

/// Go: `fnv`, FNV-1a 64 with a word-at-a-time mode.
#[derive(Debug, Clone, Copy)]
pub(super) struct Fnv(u64);

const FNV_PRIME: u64 = 1_099_511_628_211;

impl Fnv {
    /// Go: `newFnv`.
    pub(super) const fn new() -> Self {
        Self(14_695_981_039_346_656_037)
    }

    /// Go: `bytes`.
    pub(super) fn bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
    }

    /// Go: `i64`.
    pub(super) fn i64(&mut self, value: i64) {
        self.bytes(&value.to_le_bytes());
    }

    /// Go: `words`.
    pub(super) fn words(&mut self, bytes: &[u8]) {
        let (words, tail) = bytes.as_chunks::<8>();
        for word in words {
            self.0 ^= u64::from_le_bytes(*word);
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
        self.bytes(tail);
    }

    /// Go: `hex`.
    pub(super) fn hex(self) -> String {
        format!("{:016x}", self.0)
    }
}

/// Go: `bytesDigest`.
pub(super) fn bytes_digest(bytes: &[u8]) -> String {
    let mut digest = Fnv::new();
    digest.bytes(bytes);
    digest.hex()
}

/// Go: `errStr`.
pub(super) fn err_str<E: fmt::Display>(result: &Result<(), E>) -> String {
    match result {
        Ok(()) => "nil".to_owned(),
        Err(err) => format!("err:{err}"),
    }
}

/// Go: `valStr` for a read that Go performs into a nil buffer, where an empty value is nil.
pub(super) fn nil_val_str(result: &Result<Vec<u8>, CacheError>) -> String {
    match result {
        Err(err) => format!("err:{err}"),
        Ok(value) if value.is_empty() => "nil".to_owned(),
        Ok(value) => format!("len:{} fnv:{}", value.len(), bytes_digest(value)),
    }
}

/// Go: `valStr` for a value that is never nil.
pub(super) fn buf_val_str(result: &Result<&[u8], CacheError>) -> String {
    match result {
        Err(err) => format!("err:{err}"),
        Ok(value) => format!("len:{} fnv:{}", value.len(), bytes_digest(value)),
    }
}

/// Go: `counters`.
pub(super) fn counters<T: Timer>(cache: &Cache<T>) -> String {
    format!(
        "e:{} h:{} m:{} ev:{} ex:{} ow:{} to:{} aat:{} lk:{} hr:{:016x}",
        cache.entry_count(),
        cache.hit_count(),
        cache.miss_count(),
        cache.evacuate_count(),
        cache.expired_count(),
        cache.overwrite_count(),
        cache.touched_count(),
        cache.average_access_time(),
        cache.lookup_count(),
        cache.hit_rate().to_bits()
    )
}

/// The operations, in Go's `opSet...opIterate` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Set,
    Get,
    GetWithExpiration,
    Peek,
    Del,
    Touch,
    Ttl,
    GetOrSet,
    SetAndGet,
    GetFn,
    PeekFn,
    GetWithBuf,
    SetInt,
    GetInt,
    GetIntWithExpiration,
    DelInt,
    Tick,
    Clear,
    ResetStatistics,
    Iterate,
    IterStep,
}

/// Go: the operation order of the weight table.
const OPS: [Op; 21] = [
    Op::Set,
    Op::Get,
    Op::GetWithExpiration,
    Op::Peek,
    Op::Del,
    Op::Touch,
    Op::Ttl,
    Op::GetOrSet,
    Op::SetAndGet,
    Op::GetFn,
    Op::PeekFn,
    Op::GetWithBuf,
    Op::SetInt,
    Op::GetInt,
    Op::GetIntWithExpiration,
    Op::DelInt,
    Op::Tick,
    Op::Clear,
    Op::ResetStatistics,
    Op::Iterate,
    Op::IterStep,
];

/// Go: `specialTTLs`.
const SPECIAL_TTLS: [i64; 6] = [
    1 << 32,
    (1 << 32) + 1,
    (1 << 32) - 1,
    1 << 31,
    i64::MAX,
    0xFFFF_FFFF + 5,
];

/// Go: `intKeys`.
const INT_KEYS: [i64; 12] = [
    0,
    1,
    2,
    3,
    7,
    42,
    -1,
    -2,
    i64::MIN,
    i64::MAX,
    1 << 40,
    -(1 << 33),
];

/// Go: `bufCap` choices of `opGetWithBuf`.
const BUF_CAPS: [usize; 4] = [0, 1, 16, 4096];

/// Go: `scriptSpec`, parsed from a `script` golden line.
#[derive(Debug, Clone)]
pub(super) struct ScriptSpec {
    seed: u64,
    cache_size: i64,
    pub(super) ops: usize,
    pub(super) batch: usize,
    pub(super) state_every: usize,
    clock_start: u32,
    keys: usize,
    key_max_len: u64,
    small_val: u64,
    ttl_max: u64,
    tick_max: u64,
    back_permille: u64,
    collide: bool,
    huge: bool,
    weights: Vec<u64>,
    deep_keys: usize,
    small_mix: bool,
    tick_every: u64,
}

impl ScriptSpec {
    /// Go: `freecache.NewCacheCustomTimer(spec.cacheSize, clock)` in `newRunner`.
    pub(super) fn new_cache(&self) -> FakeCache {
        Cache::with_timer(
            self.cache_size,
            FakeTimer::ticking(self.clock_start, self.tick_every),
        )
    }

    /// Parses the fields after `script` (the first is the name).
    pub(super) fn parse(fields: &[&str]) -> Result<Self, Box<dyn Error>> {
        let weights = field(fields, 15)?
            .split(',')
            .map(str::parse)
            .collect::<Result<Vec<u64>, _>>()?;
        if weights.len() != OPS.len() {
            return Err(
                format!("script weights {weights:?} do not cover {} ops", OPS.len()).into(),
            );
        }
        Ok(Self {
            seed: field(fields, 1)?.parse()?,
            cache_size: field(fields, 2)?.parse()?,
            ops: field(fields, 3)?.parse()?,
            batch: field(fields, 4)?.parse()?,
            state_every: field(fields, 5)?.parse()?,
            clock_start: field(fields, 6)?.parse()?,
            keys: field(fields, 7)?.parse()?,
            key_max_len: field(fields, 8)?.parse()?,
            small_val: field(fields, 9)?.parse()?,
            ttl_max: field(fields, 10)?.parse()?,
            tick_max: field(fields, 11)?.parse()?,
            back_permille: field(fields, 12)?.parse()?,
            collide: field(fields, 13)? == "1",
            huge: field(fields, 14)? == "1",
            weights,
            deep_keys: field(fields, 16)?.parse()?,
            small_mix: field(fields, 17)? == "1",
            tick_every: field(fields, 18)?.parse()?,
        })
    }
}

/// Go: `runner`, driving a cache created by [`ScriptSpec::new_cache`].
pub(super) struct Runner<'run> {
    spec: &'run ScriptSpec,
    rng: SplitMix64,
    vrng: SplitMix64,
    clock: Arc<FakeTimer>,
    pub(super) cache: &'run FakeCache,
    keys: Vec<Vec<u8>>,
    max_kv: i64,
    /// Go: `it`, the iterator `opIterStep` advances across operations.
    it: Option<CacheIterator<'run, Arc<FakeTimer>>>,
}

impl<'run> Runner<'run> {
    /// Go: `newRunner`.
    pub(super) fn new(
        spec: &'run ScriptSpec,
        cache: &'run FakeCache,
        collisions: &[Vec<u8>],
        deep: &[Vec<u8>],
    ) -> Self {
        let size = spec.cache_size.max(512 * 1024);
        let mut runner = Self {
            spec,
            rng: SplitMix64::new(spec.seed),
            vrng: SplitMix64::new(spec.seed ^ 0x7661_6C75_6573),
            clock: Arc::clone(cache.timer()),
            cache,
            keys: Vec::new(),
            max_kv: (size / 256) / 4 - 24,
            it: None,
        };
        if spec.collide {
            runner.keys.extend(collisions.iter().cloned());
        }
        for _ in 0..spec.keys {
            let len = runner.rng.below(spec.key_max_len + 1);
            let mut key = vec![0; usize::try_from(len).unwrap_or(0)];
            runner.rng.fill(&mut key);
            runner.keys.push(key);
        }
        runner
            .keys
            .extend(deep.iter().take(spec.deep_keys).cloned());
        if spec.huge {
            runner.keys.push(vec![b'h'; 65535]);
            runner.keys.push(vec![b'h'; 65536]);
        }
        runner
    }

    /// Go: `pickOp`.
    fn pick_op(&mut self) -> Op {
        let total = self.spec.weights.iter().sum();
        let mut x = self.rng.below(total);
        for (&op, &weight) in OPS.iter().zip(&self.spec.weights) {
            if x < weight {
                return op;
            }
            x -= weight;
        }
        Op::Tick
    }

    /// Go: `pickKey`, the index of the key.
    fn pick_key(&mut self) -> usize {
        let n = u64::try_from(self.keys.len()).unwrap_or(0);
        let index = if self.rng.below(2) == 0 {
            let hot = (n / 10).max(1);
            self.rng.below(hot)
        } else {
            self.rng.below(n)
        };
        usize::try_from(index).unwrap_or(0)
    }

    /// Key `index` of the pool.
    fn key(&self, index: usize) -> &[u8] {
        self.keys.get(index).map_or(&[], Vec::as_slice)
    }

    /// Go: `pickValLen`.
    fn pick_val_len(&mut self, key_len: usize) -> i64 {
        let lim = self.max_kv - i64::try_from(key_len).unwrap_or(0);
        let small_val = self.spec.small_val;
        if self.spec.small_mix && self.rng.below(100) < 97 {
            return self.rng.below_int(small_val.cast_signed() + 1);
        }
        let x = self.rng.below(100);
        let n = if x < 60 {
            self.rng.below_int(small_val.cast_signed() + 1)
        } else if x < 75 {
            lim + self.rng.below_int(6) - 3
        } else if x < 92 {
            if lim > 0 {
                self.rng.below_int(lim + 1)
            } else {
                self.rng.below_int(small_val.cast_signed() + 1)
            }
        } else {
            self.rng.below_int(self.max_kv + 64)
        };
        n.max(0)
    }

    /// Go: `pickTTL`.
    fn pick_ttl(&mut self) -> i64 {
        let x = self.rng.below(100);
        if x < 45 {
            0
        } else if x < 50 {
            -self.rng.below_int(5) - 1
        } else if x < 88 {
            1 + self.rng.below_int(self.spec.ttl_max.cast_signed())
        } else if x < 96 {
            self.rng.below_int(1 << 20)
        } else {
            let index = self.rng.below(6);
            SPECIAL_TTLS
                .get(usize::try_from(index).unwrap_or(0))
                .copied()
                .unwrap_or(0)
        }
    }

    /// Go: `pickInt`.
    fn pick_int(&mut self) -> i64 {
        let index = self.rng.below(12);
        INT_KEYS
            .get(usize::try_from(index).unwrap_or(0))
            .copied()
            .unwrap_or(0)
    }

    /// Go: `value`.
    fn value(&mut self, len: i64) -> Vec<u8> {
        let mut value = vec![0; usize::try_from(len).unwrap_or(0)];
        self.vrng.fill(&mut value);
        value
    }

    /// Go: `step`, returning the operation's description and result.
    pub(super) fn step(&mut self) -> (String, String) {
        match self.pick_op() {
            Op::Set => self.step_set(),
            Op::GetOrSet => self.step_get_or_set(),
            Op::SetAndGet => self.step_set_and_get(),
            Op::Get => {
                let k = self.pick_key();
                let result = self.cache.get(self.key(k));
                (format!("get k{k}"), nil_val_str(&result))
            }
            Op::GetWithExpiration => {
                let k = self.pick_key();
                let result = self.cache.get_with_expiration(self.key(k));
                (format!("getexp k{k}"), expiration_str(result))
            }
            Op::Peek => {
                let k = self.pick_key();
                let result = self.cache.peek(self.key(k));
                (format!("peek k{k}"), nil_val_str(&result))
            }
            Op::Del => {
                let k = self.pick_key();
                (format!("del k{k}"), self.cache.del(self.key(k)).to_string())
            }
            Op::Touch => {
                let k = self.pick_key();
                let ttl = self.pick_ttl();
                let result = self.cache.touch(self.key(k), ttl);
                (format!("touch k{k} t{ttl}"), err_str(&result))
            }
            Op::Ttl => {
                let k = self.pick_key();
                let result = self
                    .cache
                    .ttl(self.key(k))
                    .map_or_else(|err| format!("err:{err}"), |ttl| format!("ttl:{ttl}"));
                (format!("ttl k{k}"), result)
            }
            Op::GetFn => self.step_fn("getfn"),
            Op::PeekFn => self.step_fn("peekfn"),
            Op::GetWithBuf => self.step_get_with_buf(),
            Op::SetInt => self.step_set_int(),
            Op::GetInt => {
                let ik = self.pick_int();
                (
                    format!("getint i{ik}"),
                    nil_val_str(&self.cache.get_int(ik)),
                )
            }
            Op::GetIntWithExpiration => {
                let ik = self.pick_int();
                let result = self.cache.get_int_with_expiration(ik);
                (format!("getintexp i{ik}"), expiration_str(result))
            }
            Op::DelInt => {
                let ik = self.pick_int();
                (format!("delint i{ik}"), self.cache.del_int(ik).to_string())
            }
            Op::Tick => self.step_tick(),
            Op::Clear => {
                self.cache.clear();
                ("clear".to_owned(), "ok".to_owned())
            }
            Op::ResetStatistics => {
                self.cache.reset_statistics();
                ("reset".to_owned(), "ok".to_owned())
            }
            Op::Iterate => self.step_iterate(),
            Op::IterStep => self.step_iter_step(),
        }
    }

    /// Go: `record`, an operation's result with every counter and, on a ticking clock, the
    /// number of clock reads so far.
    pub(super) fn record(&self, res: &str) -> String {
        let counters = counters(self.cache);
        if self.spec.tick_every > 0 {
            format!("{res} | {counters} c:{}", self.clock.calls())
        } else {
            format!("{res} | {counters}")
        }
    }

    /// Go: the `opIterStep` case of `step`: up to 1 to 8 more entries of the iterator kept
    /// across operations, which restarts after reaching the end.
    fn step_iter_step(&mut self) -> (String, String) {
        let k = self.rng.below(8) + 1;
        let cache = self.cache;
        let mut it = self.it.take().unwrap_or_else(|| cache.new_iterator());
        let mut digest = Fnv::new();
        let mut n = 0;
        let mut end = false;
        while n < k {
            let Some(entry) = it.next() else {
                end = true;
                break;
            };
            n += 1;
            digest.i64(i64::try_from(entry.key.len()).unwrap_or(0));
            digest.bytes(&entry.key);
            digest.i64(i64::try_from(entry.value.len()).unwrap_or(0));
            digest.bytes(&entry.value);
        }
        if !end {
            self.it = Some(it);
        }
        (
            format!("iterstep {k}"),
            format!("n:{n} end:{end} fnv:{}", digest.hex()),
        )
    }

    /// The draws shared by the `opSet`, `opGetOrSet` and `opSetAndGet` cases of Go's `step`: the
    /// key index, the value, the TTL and the description `k.. v.. t..`.
    fn pick_set(&mut self) -> (usize, Vec<u8>, i64, String) {
        let k = self.pick_key();
        let key_len = self.key(k).len();
        let vlen = self.pick_val_len(key_len);
        let ttl = self.pick_ttl();
        let value = self.value(vlen);
        (k, value, ttl, format!("k{k} v{vlen} t{ttl}"))
    }

    /// Go: the `opSet` case of `step`.
    fn step_set(&mut self) -> (String, String) {
        let (k, value, ttl, desc) = self.pick_set();
        let result = self.cache.set(self.key(k), &value, ttl);
        (format!("set {desc}"), err_str(&result))
    }

    /// Go: the `opGetOrSet` case of `step`.
    fn step_get_or_set(&mut self) -> (String, String) {
        let (k, value, ttl, desc) = self.pick_set();
        let (ret, err) = match self.cache.get_or_set(self.key(k), &value, ttl) {
            Ok(ret) => (Ok(ret.unwrap_or_default()), Ok(())),
            Err(err) => (Ok(Vec::new()), Err(err)),
        };
        (
            format!("getorset {desc}"),
            format!("{} {}", nil_val_str(&ret), err_str(&err)),
        )
    }

    /// Go: the `opSetAndGet` case of `step`.
    fn step_set_and_get(&mut self) -> (String, String) {
        let (k, value, ttl, desc) = self.pick_set();
        let (ret, result) = self.cache.set_and_get(self.key(k), &value, ttl);
        let found = ret.is_some();
        (
            format!("setandget {desc}"),
            format!(
                "{} found:{found} {}",
                nil_val_str(&Ok(ret.unwrap_or_default())),
                err_str(&result)
            ),
        )
    }

    /// Go: the `opGetFn` and `opPeekFn` cases of `step`.
    fn step_fn(&mut self, name: &str) -> (String, String) {
        let k = self.pick_key();
        let fail = self.rng.below(4) == 0;
        let mut desc = "-".to_owned();
        let record = |value: &[u8]| -> Result<(), &'static str> {
            desc = format!("len:{} fnv:{}", value.len(), bytes_digest(value));
            if fail { Err("fn error") } else { Ok(()) }
        };
        let key = self.key(k);
        let result = if name == "getfn" {
            self.cache.get_fn(key, record)
        } else {
            self.cache.peek_fn(key, record)
        };
        let err = match result {
            Ok(Ok(())) => "nil".to_owned(),
            Ok(Err(err)) => format!("err:{err}"),
            Err(err) => format!("err:{err}"),
        };
        (format!("{name} k{k} f{fail}"), format!("{desc} {err}"))
    }

    /// Go: the `opGetWithBuf` case of `step`.
    fn step_get_with_buf(&mut self) -> (String, String) {
        let k = self.pick_key();
        let index = self.rng.below(4);
        let buf_cap = BUF_CAPS
            .get(usize::try_from(index).unwrap_or(0))
            .copied()
            .unwrap_or(0);
        let mut buf = Vec::with_capacity(buf_cap);
        let result = self
            .cache
            .get_with_buf(self.key(k), &mut buf)
            .map(|()| buf.as_slice());
        (format!("getbuf k{k} c{buf_cap}"), buf_val_str(&result))
    }

    /// Go: the `opSetInt` case of `step`.
    fn step_set_int(&mut self) -> (String, String) {
        let ik = self.pick_int();
        let vlen = self.pick_val_len(8);
        let ttl = self.pick_ttl();
        let value = self.value(vlen);
        (
            format!("setint i{ik} v{vlen} t{ttl}"),
            err_str(&self.cache.set_int(ik, &value, ttl)),
        )
    }

    /// Go: the `opTick` case of `step`.
    fn step_tick(&mut self) -> (String, String) {
        let d = u32::try_from(self.rng.below(self.spec.tick_max + 1)).unwrap_or(0);
        let sign = if self.rng.below(1000) < self.spec.back_permille {
            self.clock.rewind(d);
            '-'
        } else {
            self.clock.advance(d);
            '+'
        };
        (
            format!("tick {sign}{d}"),
            format!("now:{}", self.clock.current()),
        )
    }

    /// Go: the `opIterate` case of `step`.
    fn step_iterate(&self) -> (String, String) {
        let mut digest = Fnv::new();
        let mut n = 0;
        for entry in self.cache.new_iterator() {
            n += 1;
            digest.i64(i64::try_from(entry.key.len()).unwrap_or(0));
            digest.bytes(&entry.key);
            digest.i64(i64::try_from(entry.value.len()).unwrap_or(0));
            digest.bytes(&entry.value);
        }
        ("iterate".to_owned(), format!("n:{n} fnv:{}", digest.hex()))
    }
}

/// Go: the result of `GetWithExpiration` and `GetIntWithExpiration` in `step`, with expiration 0
/// on an error.
fn expiration_str(result: Result<(Vec<u8>, u32), CacheError>) -> String {
    let (value, exp) = match result {
        Ok((value, exp)) => (Ok(value), exp),
        Err(err) => (Err(err), 0),
    };
    format!("{} exp:{exp}", nil_val_str(&value))
}

/// Writes a script's records for one batch next to the system temp dir, for diffing with the Go
/// generator's `-trace` output, and returns the path.
pub(super) fn write_trace(name: &str, batch: usize, records: &[String]) -> String {
    let path = env::temp_dir().join(format!("pbs-freecache-{name}-batch{batch}.trace"));
    let mut text = String::new();
    for record in records {
        text.push_str(record);
        text.push('\n');
    }
    match fs::write(&path, text) {
        Ok(()) => path.display().to_string(),
        Err(err) => format!("(could not write the trace: {err})"),
    }
}
