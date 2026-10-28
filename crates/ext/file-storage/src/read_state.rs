//! Shared read accounting keeps resource limits independent of integrity checks.
use crate::{ByteRange, ObjectInfo, ReadOptions, StorageError};
use sha2::{Digest, Sha256};
use std::io;

pub(crate) struct ReadState {
    size: u64,
    hash: Option<Sha256>,
    options: ReadOptions,
    limit: u64,
}

impl ReadState {
    pub(crate) fn new(
        options: ReadOptions,
        adapter_limit: u64,
        inspect: bool,
    ) -> Result<Self, StorageError> {
        options.expected.validate()?;
        let limit = options
            .max_bytes
            .map_or(adapter_limit, |limit| limit.min(adapter_limit));
        if let Some(range) = options.range {
            if options.expected.size_bytes.is_some() || options.expected.sha256.is_some() {
                return Err(StorageError::InvalidInput(
                    "ranged reads cannot verify full-object integrity".into(),
                ));
            }
            if range.offset.checked_add(range.length).is_none() {
                return Err(beyond_end());
            }
            if range.length > limit {
                return Err(StorageError::SizeLimit { limit });
            }
        }
        Ok(Self {
            size: 0,
            hash: (inspect || options.expected.sha256.is_some()).then(Sha256::new),
            limit,
            options,
        })
    }

    pub(crate) fn range(&self) -> Option<ByteRange> {
        self.options.range
    }

    /// Bytes still owed by a ranged read.
    pub(crate) fn remaining(&self) -> Option<u64> {
        self.options
            .range
            .map(|range| range.length.saturating_sub(self.size))
    }

    pub(crate) fn update(&mut self, bytes: &[u8]) -> Result<(), StorageError> {
        self.size = self
            .size
            .checked_add(bytes.len() as u64)
            .filter(|size| *size <= self.limit)
            .ok_or(StorageError::SizeLimit { limit: self.limit })?;
        if self
            .options
            .range
            .is_some_and(|range| self.size > range.length)
        {
            return Err(StorageError::Io {
                operation: "read object range",
                source: io::Error::other("more bytes than the requested range"),
            });
        }
        if let Some(hash) = &mut self.hash {
            hash.update(bytes);
        }
        Ok(())
    }

    pub(crate) fn finish(&self) -> Result<(), StorageError> {
        // Adapters check ranges against the object's size before streaming, so
        // a short range is a downstream failure, not invalid input.
        if self.remaining().is_some_and(|remaining| remaining > 0) {
            return Err(StorageError::Io {
                operation: "read object range",
                source: io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "fewer bytes than the requested range",
                ),
            });
        }
        let expected = &self.options.expected;
        if expected.size_bytes.is_some_and(|size| size != self.size)
            || expected.sha256.as_ref().is_some_and(|hash| {
                hash != &hex::encode(
                    self.hash
                        .as_ref()
                        .expect("checksum requested")
                        .clone()
                        .finalize(),
                )
            })
        {
            return Err(StorageError::IntegrityMismatch);
        }
        Ok(())
    }

    pub(crate) fn info(&self) -> ObjectInfo {
        ObjectInfo {
            size_bytes: self.size,
            sha256: hex::encode(
                self.hash
                    .as_ref()
                    .expect("inspection hashes bytes")
                    .clone()
                    .finalize(),
            ),
        }
    }
}

pub(crate) fn beyond_end() -> StorageError {
    StorageError::InvalidInput("range beyond end of object".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExpectedIntegrity;

    #[test]
    fn default_and_size_only_reads_do_not_hash() {
        for expected_size in [None, Some(3)] {
            let options = ReadOptions {
                expected: ExpectedIntegrity {
                    size_bytes: expected_size,
                    sha256: None,
                },
                ..Default::default()
            };
            let mut state = ReadState::new(options, 100, false).unwrap();
            state.update(b"abc").unwrap();
            state.finish().unwrap();
            assert!(state.hash.is_none());
        }
    }

    #[test]
    fn ranged_reads_reject_short_and_long_bodies_as_io() {
        let options = ReadOptions {
            range: Some(ByteRange {
                offset: 5,
                length: 3,
            }),
            ..Default::default()
        };
        let mut short = ReadState::new(options.clone(), 100, false).unwrap();
        short.update(b"ab").unwrap();
        assert!(matches!(short.finish(), Err(StorageError::Io { .. })));
        let mut long = ReadState::new(options, 100, false).unwrap();
        assert!(matches!(long.update(b"abcd"), Err(StorageError::Io { .. })));
    }
}
