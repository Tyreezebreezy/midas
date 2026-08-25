# Release standard

1. Finish and checkpoint the verified release candidate.
2. Run deterministic regression.
3. Run security and privacy gates.
4. Verify packaging and installer behavior.
5. Resolve or explicitly accept known limitations.
6. Align application and package versions.
7. Build release artifacts from the approved commit.
8. Calculate SHA-256 hashes with `scripts/hash-artifacts.ps1`.
9. Obtain the owner release gate.
10. Create an annotated Git tag.
11. Push `main` and the tag to the private Gitea remote.
12. Create a concise Gitea release listing artifact names, sizes, hashes, prerequisites, and limitations.
13. Download and verify published artifacts.
14. Verify final repository and release state.
15. Stop. Do not begin the next version automatically. Never publish a `v1.0.0` release without explicit owner approval.

Publishing, tagging, pushing, and final release acceptance are never implied by implementation approval.
