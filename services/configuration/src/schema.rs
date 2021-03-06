// Copyright 2018 The Exonum Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Storage schema for the configuration service.

use exonum::crypto::{self, CryptoHash, Hash, PublicKey, Signature};
use exonum::storage::{Fork, ProofListIndex, ProofMapIndex, Snapshot, StorageValue};

use std::borrow::Cow;

use transactions::{Propose, Vote};

// Defines `&str` constants with given name and value.
macro_rules! define_names {
    ($($name:ident => $value:expr;)+) => (
        $(const $name: &str = concat!("configuration.", $value);)*
    )
}

define_names! {
    PROPOSES => "proposes";
    PROPOSE_HASHES => "propose_hashes";
    VOTES => "votes";
}

encoding_struct! {
    /// Extended information about a proposal used for the storage.
    struct ProposeData {
        /// Proposal transaction.
        tx_propose: Propose,
        /// Merkle root of all votes for the proposal.
        votes_history_hash: &Hash,
        /// Number of eligible voting validators.
        num_validators: u64,
    }
}

lazy_static! {
    static ref NO_VOTE_BYTES: Vec<u8> = Vote::new_with_signature(
        &PublicKey::zero(),
        &Hash::zero(),
        &Signature::zero(),
    ).into_bytes();
}

/// A functional equivalent to `Option<Vote>` used to store votes in the service schema.
///
/// # Notes
///
/// The `None` variant of the type is represented by a `Vote` with all bytes set to zero.
#[derive(Clone, Debug, PartialEq)]
pub struct MaybeVote(Option<Vote>);

impl MaybeVote {
    /// Creates a `None` variant.
    pub fn none() -> Self {
        MaybeVote(None)
    }

    /// Creates a `Some` variant.
    pub fn some(vote: Vote) -> Self {
        MaybeVote(Some(vote))
    }
}

impl From<MaybeVote> for Option<Vote> {
    fn from(value: MaybeVote) -> Option<Vote> {
        value.0
    }
}

impl ::std::ops::Deref for MaybeVote {
    type Target = Option<Vote>;

    fn deref(&self) -> &Option<Vote> {
        &self.0
    }
}

impl CryptoHash for MaybeVote {
    fn hash(&self) -> Hash {
        match self.0 {
            Some(ref vote) => vote.hash(),
            None => crypto::hash(&NO_VOTE_BYTES),
        }
    }
}

impl StorageValue for MaybeVote {
    fn into_bytes(self) -> Vec<u8> {
        match self.0 {
            Some(vote) => vote.into_bytes(),
            None => NO_VOTE_BYTES.clone(),
        }
    }

    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        if NO_VOTE_BYTES.as_slice().eq(bytes.as_ref()) {
            MaybeVote::none()
        } else {
            MaybeVote::some(Vote::from_bytes(bytes))
        }
    }
}

/// Database schema used by the configuration service.
#[derive(Debug)]
pub struct Schema<T> {
    view: T,
}

impl<T> Schema<T>
where
    T: AsRef<Snapshot>,
{
    /// Creates a new schema.
    pub fn new(snapshot: T) -> Schema<T> {
        Schema { view: snapshot }
    }

    /// Returns propose information indexed by the hash of the configuration corresponding
    /// to a proposal.
    ///
    /// Consult [the crate-level docs](index.html) for details how hashes of the configuration
    /// are calculated.
    pub fn propose_data_by_config_hash(&self) -> ProofMapIndex<&Snapshot, Hash, ProposeData> {
        ProofMapIndex::new(PROPOSES, self.view.as_ref())
    }

    /// Returns a table of hashes of proposed configurations in the commit order.
    pub fn config_hash_by_ordinal(&self) -> ProofListIndex<&Snapshot, Hash> {
        ProofListIndex::new(PROPOSE_HASHES, self.view.as_ref())
    }

    /// Returns a table of votes of validators for a particular proposal, referenced
    /// by its configuration hash.
    pub fn votes_by_config_hash(&self, config_hash: &Hash) -> ProofListIndex<&Snapshot, MaybeVote> {
        ProofListIndex::new_in_family(VOTES, config_hash, self.view.as_ref())
    }

    /// Returns a `Propose` transaction with a particular configuration hash.
    pub fn propose(&self, cfg_hash: &Hash) -> Option<Propose> {
        self.propose_data_by_config_hash()
            .get(cfg_hash)?
            .tx_propose()
            .into()
    }

    /// Returns a list of votes for the proposal corresponding to the given configuration hash.
    #[cfg_attr(feature = "cargo-clippy", allow(let_and_return))]
    pub fn votes(&self, cfg_hash: &Hash) -> Vec<Option<Vote>> {
        let votes_by_config_hash = self.votes_by_config_hash(cfg_hash);
        let votes = votes_by_config_hash.iter().map(MaybeVote::into).collect();
        votes
    }

    /// Returns state hash values used by the configuration service.
    pub fn state_hash(&self) -> Vec<Hash> {
        vec![
            self.propose_data_by_config_hash().merkle_root(),
            self.config_hash_by_ordinal().merkle_root(),
        ]
    }
}

impl<'a> Schema<&'a mut Fork> {
    /// Mutable version of the `propose_data_by_config_hash` index.
    pub(crate) fn propose_data_by_config_hash_mut(
        &mut self,
    ) -> ProofMapIndex<&mut Fork, Hash, ProposeData> {
        ProofMapIndex::new(PROPOSES, &mut self.view)
    }

    /// Mutable version of the `config_hash_by_ordinal` index.
    pub(crate) fn config_hash_by_ordinal_mut(&mut self) -> ProofListIndex<&mut Fork, Hash> {
        ProofListIndex::new(PROPOSE_HASHES, &mut self.view)
    }

    /// Mutable version of the `votes_by_config_hash` index.
    pub(crate) fn votes_by_config_hash_mut(
        &mut self,
        config_hash: &Hash,
    ) -> ProofListIndex<&mut Fork, MaybeVote> {
        ProofListIndex::new_in_family(VOTES, config_hash, &mut self.view)
    }
}

#[cfg(test)]
mod tests {
    use exonum::storage::{Database, MemoryDB};
    use super::*;

    lazy_static! {
        static ref NO_VOTE: Vote = Vote::new_with_signature(
            &PublicKey::zero(),
            &Hash::zero(),
            &Signature::zero(),
        );
    }

    /// Check compatibility of old and new implementations of "absence of vote" signaling.
    #[test]
    fn test_serialization_of_maybe_vote() {
        const VALIDATORS: usize = 5;

        assert_eq!(NO_VOTE.hash(), MaybeVote::none().hash());
        assert_eq!(NO_VOTE.clone().into_bytes(), MaybeVote::none().into_bytes());

        let (pubkey, key) = crypto::gen_keypair();
        let vote = Vote::new(&pubkey, &Hash::new([1; 32]), &key);
        assert_eq!(
            vote.clone().into_bytes(),
            MaybeVote::some(vote.clone()).into_bytes()
        );
        assert_eq!(vote.hash(), MaybeVote::some(vote.clone()).hash());

        let db = MemoryDB::new();
        let mut fork = db.fork();
        let merkle_root = {
            let mut index: ProofListIndex<_, Vote> = ProofListIndex::new("index", &mut fork);
            for _ in 0..VALIDATORS {
                index.push(NO_VOTE.clone());
            }
            index.set(1, vote.clone());
            index.merkle_root()
        };
        db.merge(fork.into_patch()).unwrap();

        let snapshot = db.snapshot();
        let index: ProofListIndex<_, MaybeVote> = ProofListIndex::new("index", &snapshot);
        for (i, stored_vote) in index.iter().enumerate() {
            assert_eq!(
                stored_vote,
                if i == 1 {
                    MaybeVote::some(vote.clone())
                } else {
                    MaybeVote::none()
                }
            );
        }

        // Touch the index in order to recalculate its root hash
        let new_merkle_root = {
            let mut fork = db.fork();
            let mut index: ProofListIndex<_, MaybeVote> = ProofListIndex::new("index", &mut fork);
            index.set(2, MaybeVote::some(vote.clone()));
            index.set(2, MaybeVote::none());
            index.merkle_root()
        };
        assert_eq!(merkle_root, new_merkle_root);
    }
}
