# UniFlow Transport Boundary — Client Contract

> The protocol/transport boundary that a future **Flutter / Swift / Kotlin** mobile
> client (or any remote agent) drives. Mobile-native runtimes are out of scope for
> code, but this contract is intentionally clean enough that such a client could
> implement either side without touching the engine internals.

## The boundary: two random-access, integrity-checked stream traits

The parallel core (`infrastructure::transfer::parallel`) speaks only to two traits
(`infrastructure::transfer::adapters`):

```rust
pub trait ChunkSource: Send + Sync {
    fn len(&self) -> u64;
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize>;
}
pub trait ChunkSink: Send + Sync {
    fn write_at(&self, offset: u64, data: &[u8]) -> Result<()>;
    fn sync(&self) -> Result<()>;
}
```

Everything the engine needs from an endpoint is: **its length**, the ability to
**read a byte range**, and (on the destination) the ability to **write a byte range**
and **durably flush**. That is the entire contract. A device client implements
`ChunkSource` (to send a file) and/or `ChunkSink` (to receive one); the cloud
adapter implements both over the rclone bridge; the P2P adapter implements both over
iroh/QUIC streams.

## Wire framing (what a remote client must speak)

For a *networked* endpoint the engine's per-chunk pipeline is:

```
sender:    read_at → BLAKE3(plaintext) → [zstd compress] → [AEAD encrypt] → frame → send
receiver:  recv → deframe → [AEAD decrypt] → [zstd decompress] → BLAKE3 verify → write_at
```

A frame is self-describing so the receiver can place it without shared state:

| field            | bytes | meaning                                              |
|------------------|-------|------------------------------------------------------|
| `offset`         | 8 LE  | absolute byte offset in the file                     |
| `plain_len`      | 4 LE  | original (decoded) length of this chunk              |
| `wire_len`       | 4 LE  | length of the `payload` that follows                 |
| `flags`          | 1     | bit0 = compressed, bit1 = encrypted                  |
| `nonce`          | 12    | AEAD nonce (present iff encrypted)                   |
| `blake3`         | 32    | hash of the **plaintext** chunk (integrity)          |
| `payload`        | wire_len | compressed-then-encrypted bytes                   |

Negotiation (chunk size, stream count, codecs, encryption choice) is the output of
the **Planner** (`TransferPlan`) and is sent once at session start; the client does
not need to re-derive it. AES-GCM vs ChaCha20 is chosen by the planner from both
peers' hardware (`PairProfile.both_have_aes_hw()`).

## Session lifecycle a client implements

1. **Hello / profile** — client reports its `EndpointProfile` (CPU/SIMD/AES, RAM,
   storage class, OS/FS facts) so the planner can tune the pair. (A minimal client
   may report only cores + AES capability; the planner degrades gracefully.)
2. **Plan** — engine returns the `TransferPlan` (chunk size, streams, codecs, etc.).
3. **Transfer** — `stream_count` concurrent streams carry frames; the receiver
   writes each at its `offset`. The AIMD controller may adjust in-flight depth.
4. **Resume** — on reconnect the client sends the highest **contiguous** byte offset
   it has durably written; the engine resends only frames beyond it.
5. **Finalize** — after the last frame, an end-to-end BLAKE3 root is exchanged and
   verified; only then is the destination atomically published (temp → rename).

## Cross-platform correctness the client can rely on

All path/case/permission/timestamp reconciliation happens in ONE place
(`infrastructure::transfer::normalize`), driven by the `OsFsInfo` each side
reported. A client therefore sends **relative POSIX-style paths** and plaintext
metadata; the engine normalizes them for the destination FS (separator conversion,
Windows reserved-name and illegal-char rejection, case-collision detection,
timestamp-resolution rounding) and fails loudly rather than writing a mangled or
colliding file. See `docs/architecture.md` for the normalization rules.

## Guarantees

- **Integrity**: every chunk is BLAKE3-checked on arrival; the whole file is
  re-verified before publish.
- **Atomicity**: the destination is never partially overwritten — it appears in one
  rename after full verification.
- **Resumability**: a dropped session resumes from the last contiguous offset.
- **Confidentiality**: when the policy requests it, payloads are AEAD-encrypted
  end-to-end; the relay/daemon sees only ciphertext (zero-knowledge preserved).
- **Graceful degradation**: a client that cannot compress, cannot do AES, or has one
  stream still completes — the planner simply selects a simpler plan.
