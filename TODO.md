# TODO
1. [ ] Per network settings (max blocks age/number - for pruning)
4. [ ] Multiple RPCs support
5. [ ] Telemtry (add telemetry-batteries)
   1. [ ] Metrics
   2. [ ] Tracing
6. [ ] Automated testing
   1. [x] Basic
   2. [ ] Basic with contracts
   3. [ ] Escalation testing
   4. [ ] Reorg testing (how?!?)
7.  [ ] Plug block holes - we can periodically fetch the list of known blocks for a given chain and find and fetch any missing ones from the RPC
8.  [ ] Find missing txs - sometimes a transaction can be sent but not saved in the DB. On every block we should fetch all the txs (not just hashes) and find txs coming from our relayer addresses. This way we can find missing transactions.
9.  [ ] Smarter broadcast error handling - we shouldn't constantly attempt to broadcast the same tx if it's failing (e.g. because relayer address is out of funds).

# IN PROGRESS
1. [ ] Design authentication

# DONE
1. [x] Parallelization:
    1. [x] Parallelize block indexing - depends on per network settings
    2. [x] Parallelize nonce updating
    3. [x] Parallelize broadcast per ~chain id~ relayer id
4. [x] No need to insert all block txs into DB
5. [x] Prune block info
6. [x] RPC Access
5. [x] Cross-network dependencies (i.e. Optimism depends on L1 gas cost)
6. [x] Transaction priority
7. [x] Handling reorgs
8. [x] Per relayer settings (i.e. max inflight txs, max gas price)
