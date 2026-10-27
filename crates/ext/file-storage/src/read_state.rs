//! Shared read accounting keeps resource limits independent of integrity checks.
use crate::{ObjectInfo, ReadOptions, StorageError};
use sha2::{Digest, Sha256};

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
        Ok(Self {
            size: 0,
            hash: (inspect || options.expected.sha256.is_some()).then(Sha256::new),
            limit: options
                .max_bytes
                .map_or(adapter_limit, |limit| limit.min(adapter_limit)),
            options,
        })
    }

    pub(crate) fn update(&mut self, bytes: &[u8]) -> Result<(), StorageError> {
        self.size = self
            .size
            .checked_add(bytes.len() as u64)
            .filter(|size| *size <= self.limit)
            .ok_or(StorageError::SizeLimit { limit: self.limit })?;
        if let Some(hash) = &mut self.hash {
            hash.update(bytes);
        }
        Ok(())
    }

    pub(crate) fn finish(&self) -> Result<(), StorageError> {
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
}
