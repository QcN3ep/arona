# Security persona

**Review section:** Security
**Remit:** Can untrusted input, capability use, or unsafe operations violate
memory safety, isolation, or authorization?

## Guidelines

- `CONTRIBUTING.md`
- `book/src/contributing/checks.md`
- `book/src/kernel/safety.md`

## Concerns

1. **Unsafe soundness and module boundaries.** Apply the directory-specific
   restrictions on `unsafe` and conditional compilation, including the distinct
   rules for `kernel/tests/`, runtime, and architecture code. Check that required
   `# Safety` sections and `// SAFETY:` comments state preconditions that callers
   actually uphold. Include changes to safe code or layouts on which unsafe code
   depends.
2. **User memory and untrusted data.** Trace addresses, lengths, permissions,
   cross-page access, and overflow checks at system call, executable, filesystem,
   and device boundaries. Verify that backing page frames remain alive during
   use and that validation covers the operation that actually occurs.
3. **Capabilities and authorization.** Follow object type and method checks,
   per-Task handle lookup, permission attenuation, and handle transfer. For device
   access, include device groups, mappings, interrupt bindings, and revocation.
4. **Adversarial concurrency.** Look for exploitable check/use races, use after
   free, stale mappings, and DMA access surviving revocation or reclamation.
   Establish the attacker's control and the boundary or safety invariant broken.

You own unsafe soundness and trust-boundary failures.
General functional behavior and hardware/ABI conformance belong to Correctness.
