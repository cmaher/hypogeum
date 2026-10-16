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

Reads verify expected size/hash only after successful EOF. Partial consumption
does not establish integrity. `inspect` hashes the complete object. Missing
objects are distinct from malformed references, permission failures and corrupt
bytes. A failure after installation or unlink may report uncertain completion;
do not delete a potentially completed object as an automatic retry strategy.

The contract has no general listing, rename, resume or distributed transaction
operation. Persist references in the owning service if durable tracking is needed.

`cargo test -p file-storage` exercises the generic contract and filesystem
failures using temporary directories and synthetic bytes; it needs no database.
