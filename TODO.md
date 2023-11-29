# TODO

1. [x] Handling reorgs
2. [ ] Per network settings (max blocks age/number - for pruning)
3. [x] Per relayer settings (i.e. max inflight txs, max gas price)
4. [ ] Multiple RPCs support
5. [x] RPC Access
6. [x] Cross-network dependencies (i.e. Optimism depends on L1 gas cost)
7. [x] Transaction priority
8. [ ] Metrics
9. [ ] Tracing (add telemetry-batteries)
10. [ ] Automated testing
   1. [x] Basic
   2. [ ] Basic with contracts
   3. [ ] Escalation testing
   4. [ ] Reorg testing (how?!?)
11. [x] Parallelization:
    1.  [x] Parallelize block indexing - depends on per network settings
    2.  [x] Parallelize nonce updating
    3.  [x] Parallelize broadcast per ~chain id~ relayer id
12. [x] No need to insert all block txs into DB
13. [x] Prune block info
14. [ ] Authentication
15. [ ] Plug block holes - we can periodically fetch the list of known blocks for a given chain and find and fetch any missing ones from the RPC
