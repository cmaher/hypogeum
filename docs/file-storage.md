# Streamed byte storage

`FileStorage` stores bounded byte streams at caller-chosen keys and returns
object references plus actual size and SHA-256. It supports write, read, inspect
and idempotent delete. Constructors receive configuration explicitly. Services
decide when objects should be created, referenced or deleted, and which keys
they use.

## Object keys

An `ObjectKey` is a path-shaped key such as `photos/2026/a.jpg`, validated on
construction (`ObjectKey::new`, `TryFrom`, `FromStr` or deserialization):

- non-empty, relative, `/`-separated segments;
- no empty, `.` or `..` segments, so no leading or trailing `/` and no `//`;
- no backslashes, NUL or other control characters;
- at most 255 bytes per segment and 1,024 bytes per key.

Keys have S3-like semantics. `write(&key, input, expected)` creates the object
at `key` or atomically replaces the existing one: readers observe the previous
or the new object, never a partial one. Integrity checks run before the new
object is installed, so a failed write leaves any previous object in place.
Callers can place objects deliberately, for example a derived file next to its
source (`a/b/c.jpg` and `a/b/c.thumb.jpg`). There is no listing or prefix
enumeration.

`ObjectRef` identifies a reference version, store alias and key. Version 2
references carry `ObjectKey`-valid keys; build them with `ObjectRef::new` or take
them from `StoredObject::object_ref`. Adapters reject other versions (including
version 1, whose adapter-generated keys are not migrated), other stores and
malformed keys with `StorageError::InvalidReference`. Preserve store aliases when
moving or restoring storage.

| Key | Filesystem adapter | Google Drive adapter |
| --- | --- | --- |
| `c.jpg` | `<root>/c.jpg` | file `c.jpg` in the storage folder |
| `a/b/c.jpg` | `<root>/a/b/c.jpg` | file `c.jpg` in folder `b` in folder `a` in the storage folder |

## Filesystem adapter

Create an application-controlled root directory, then construct
`FilesystemStorage::new(FilesystemConfig { root, store_id, max_object_bytes })`.
The adapter initializes a `staging` directory on the root's filesystem. That
name is reserved: a key whose first segment is `staging` is rejected. Its
durability profile requires Unix directory synchronization and atomic rename;
unsupported platforms fail explicitly. Roots and parent directories must remain
under application control; concurrent hostile filesystem mutation is outside
the adapter contract.

Writes stream to staging, enforce the size limit, verify supplied integrity and
sync bytes. They then create and sync missing parent directories, install the
object by rename (replacing any existing regular file) and sync the parent and
staging directory entries. Readers that opened the previous object keep reading
its bytes. Symlinks and non-regular files are never followed or replaced: a
symlinked key segment, or a symlink, directory or FIFO at the key, is rejected.
A key below an existing object (`a/b` when `a` is a file) cannot be written and
reads as missing. Deletion removes only the object; emptied parent directories
remain. Cancellation does not expose partial objects. Failed cleanup can leave
staging debris. Initialization does not remove that debris automatically.

Roots used before path keys hold objects at `completed/<uuid>`. They remain
readable through version 2 references with key `completed/<uuid>`; consumers
migrate their own references.

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

Set `ReadOptions::range` to stream only `[offset, offset + length)`. The whole
range must lie within the object; a range extending past the end returns
`StorageError::InvalidInput`, and so does combining a range with `expected`
checks, because a slice cannot verify full-object integrity. `max_bytes` and the
adapter limit apply to `length`, and a zero `length` returns an empty stream. A
body shorter or longer than the range returns `StorageError::Io`. The Drive
adapter sends a `Range` header and streams only a `206` whose `Content-Range`
matches; a server that ignores `Range` yields `StorageError::Io` rather than the
whole object.

```rust,ignore
let slice = storage.read(&reference, ReadOptions {
    range: Some(ByteRange { offset: 4_096, length: 1_024 }),
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

## Google Drive paths and cache

Each key segment but the last is a folder below the configured storage folder;
the last is the file name. Writes create missing folders and then either create
the file or upload a new revision to the existing file ID, which Drive makes
visible only once the upload completes. If Drive's reported size or checksum
does not match the uploaded bytes, the adapter deletes that file, including a
replaced one, rather than leave unverified bytes at the key.

Reads, inspections and deletes resolve the key from the storage folder by name
(`name = '<segment>' and '<parent>' in parents and trashed = false`, with quotes
and backslashes escaped). Folders only match folder segments and files only the
last segment, so a file and a folder may share a name. Trashed items and items
outside the storage folder never match; shortcuts are not objects, and a write
to a key held by a shortcut is rejected. Duplicate names at one segment are
ambiguous and return `StorageError::InvalidInput` rather than choosing one.
This client serializes its own folder creation, but separate clients creating
the same new folder or file concurrently can still produce duplicates.

Resolved folder IDs, file IDs and reported file sizes are cached with Moka,
bounded by `GoogleDriveConfig::path_cache.capacity` (default 4,096 entries) and
`path_cache.ttl` (default ten minutes from insertion). Cache hits do not extend
the TTL. A zero capacity or TTL disables the cache, and a TTL beyond 1,000 years
is rejected. Repeated reads of a cached key skip path resolution, and ranged
reads use the cached size for the beyond-end check. When Drive answers 404 for a
cached ID, 416 for a cached range, or a cached size rejects a range, the adapter
evicts the path and resolves it again once. Other clients' renames, moves and
trash changes may remain unseen until the TTL expires.
