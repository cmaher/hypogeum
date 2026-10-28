# Streamed byte storage

`FileStorage` accepts bounded byte streams and returns opaque object references
plus actual size and SHA-256. It supports write, read, inspect and idempotent
delete. Constructors receive configuration explicitly. Services decide when
objects should be created, referenced or deleted.

`ObjectRef` identifies a reference version, store alias and adapter-owned key.
Preserve store aliases when moving or restoring storage. Callers must not
interpret keys as filesystem paths. The filesystem adapter accepts only its
canonical UUID keys and rejects unknown reference versions and stores.

## Filesystem adapter

Create an application-controlled root directory, then construct
`FilesystemStorage::new(FilesystemConfig { root, store_id, max_object_bytes })`.
The adapter initializes staging/completed directories on the same filesystem.
Its durability profile requires Unix directory synchronization and hard links;
unsupported platforms fail explicitly. Roots and parent directories must remain
under application control; concurrent hostile filesystem mutation is outside
the adapter contract.

Writes stream to staging, enforce the size limit, verify supplied integrity,
sync bytes, install without overwriting and sync directory entries. Cancellation
does not expose partial objects. Failed cleanup can leave staging debris.
Initialization does not remove that debris automatically.

Reads use `read(&reference, ReadOptions::default())` to stream bytes without
integrity verification or SHA-256 computation. `ReadOptions` replaces the previous
`ExpectedIntegrity` argument for reads; writes still take `ExpectedIntegrity` and
return actual size and SHA-256.

Set `ReadOptions::expected.size_bytes` and/or `ReadOptions::expected.sha256` to
explicitly request full-object verification. Size-only checks do not hash bytes.
Requested checks run at EOF and mismatches produce `StorageError::IntegrityMismatch`
instead of successful EOF. Partial consumption does not establish integrity.
`inspect` always reads and hashes the complete object.

Resource limits apply independently of integrity verification. Both adapters always
enforce their configured `max_object_bytes`; `ReadOptions::max_bytes` can impose a
smaller per-read limit without an expected size or checksum. The smaller limit wins,
and exceeding it returns `StorageError::SizeLimit`. A zero per-read limit permits
only empty objects. For example:

```rust,ignore
let bytes = storage.read(&reference, ReadOptions::default()).await?;
let bounded = storage.read(&reference, ReadOptions {
    max_bytes: Some(1_048_576),
    ..Default::default()
}).await?;
let verified = storage.read(&reference, ReadOptions {
    expected: ExpectedIntegrity {
        size_bytes: Some(saved_info.size_bytes),
        sha256: Some(saved_info.sha256),
    },
    ..Default::default()
}).await?;
```

Missing objects are distinct from malformed references, permission failures and corrupt
bytes. A failure after installation or unlink may report uncertain completion;
do not delete a potentially completed object as an automatic retry strategy.

The contract has no general listing, rename, resume or distributed transaction
operation. Persist references in the owning service if durable tracking is needed.

`cargo test -p hypgmx-file-storage` exercises the generic contract and filesystem
failures using temporary directories and synthetic bytes; it needs no database.

Enable `--features google-drive` to run the same contract cases against the Drive
adapter using a local protocol fixture, without credentials. Both adapters use the
same read accounting and opt-in integrity policy.

## Google Drive ancestry cache

The Drive adapter accepts files anywhere below its configured root folder. It
caches verified ancestor folder IDs with Moka, with a maximum capacity of 4,096
entries and a ten-minute time to live from insertion. Cache hits do not extend
the TTL. Ancestor moves and trash changes are not observed while cached ancestry
is trusted; the target file's own metadata is checked on every operation.
