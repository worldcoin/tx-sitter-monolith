# TODO

1. [ ] Handling reorgs
2. [ ] Per network settings (i.e. max inflight txs, max gas price, block time)
3. [ ] Multiple RPCs support
4. [ ] Cross-network dependencies (i.e. Optimism depends on L1 gas cost)
5. [ ] Transaction priority
6. [ ] Metrics
7. [ ] Tracing (add telemetry-batteries)
8. [ ] Automated testing
   1. [x] Basic
   2. [ ] Basic with contracts
   3. [ ] Escalation testing
   4. [ ] Reorg testing (how?!?)
9.  [ ] Parallelization:
    1.  [ ] Parallelize block indexing - depends on per network settings
    2.  [x] Parallelize nonce updating
    3.  [ ] Parallelize broadcast per chain id
10. [x] No need to insert all block txs into DB
11. [x] Prune block info
