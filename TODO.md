# TODO

1. [x] Handling reorgs
2. [ ] Per network settings - is this still needed?
3. IN PROGRESS [ ] Per relayer settings (i.e. max inflight txs, max gas price)
4. [ ] Multiple RPCs support
5. [ ] Cross-network dependencies (i.e. Optimism depends on L1 gas cost)
6. [ ] Transaction priority
7. [ ] Metrics
8. [ ] Tracing (add telemetry-batteries)
9. [ ] Automated testing
   1. [x] Basic
   2. [ ] Basic with contracts
   3. [ ] Escalation testing
   4. [ ] Reorg testing (how?!?)
10. [x] Parallelization:
    1.  [x] Parallelize block indexing - depends on per network settings
    2.  [x] Parallelize nonce updating
    3.  [ ] Parallelize broadcast per chain id
11. [x] No need to insert all block txs into DB
12. [x] Prune block info
13. [ ] Authentication

