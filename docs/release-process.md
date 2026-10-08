# Branch and release process

This document describes the repository's mainline contribution flow and the evidence required to
publish JavaScriptCore artifacts. It does not authorize a product release by itself. A green pull
request, main-branch run, or validation artifact is not publication approval.

At the current checked-in state, all four supported JSC targets are still `planned`, so no published
JSC artifact is implied. The `0.1` Runtime preview also remains subject to the M3 acceptance gate.

## Branch and pull request

`main` is the integration branch. Work on a short-lived branch based on the current `main`, using a
descriptive prefix and, when applicable, the issue number:

```text
feat/49-fetch-streaming
fix/50-shutdown-race
docs/release-process
ci/release-validation
```

Keep each branch focused and open a pull request targeting `main`. The pull request must pass
`Check Rust` and **M2 required gate**, receive review, and enter the merge queue. Merge through
**Merge when ready**; do not merge directly or bypass the queue. Delete the short-lived branch after
merge. This repository does not use long-lived release branches; create one only if maintainers
approve and document a specific support requirement.

Branch protection, merge-queue requirements, and tag protection are repository settings, not
properties enforced by a workflow file. Maintainers must keep the `M2 required gate` check required
and prevent release-tag updates or deletion.

## JavaScriptCore artifact release

The platform builders produce candidate artifacts. They do not publish them. Use this sequence for
an artifact release:

1. **Select a reviewed candidate.** Merge the intended changes to `main` and record the full commit
   SHA. Create an immutable, reviewed tag at that exact commit only after review; never move or
   recreate the tag. A candidate tag must point to a commit reachable from `main`.
2. **Run release validation at that tag.** Dispatch the combined M2 workflow—not independent
   platform runs—so all four targets are validated at the same SHA, with cold independent rebuilds
   enabled:

   ```bash
   reviewed_tag=YOUR_IMMUTABLE_REVIEWED_TAG
   git rev-parse "$reviewed_tag^{commit}"
   gh workflow run m2.yml --ref "$reviewed_tag" -f compare_rebuild=true
   gh run list --workflow m2.yml --event workflow_dispatch
   gh run view RUN_ID --json headSha,conclusion,jobs,url
   gh run download RUN_ID --pattern 'm2-*' --dir evidence/m2
   gh run download RUN_ID --pattern 'kunlun-jsc-*' --dir evidence/jsc
   python3 distribution/jsc/scripts/m2_gate.py summarize \
     --evidence evidence/m2 \
     --commit "$(git rev-parse "$reviewed_tag^{commit}")" \
     --output evidence/m2-summary.md
   ```

   Confirm the run's full `headSha` matches the tag, the combined gate and Miri passed, all four
   platform reports are present, each independent rebuild is byte-identical, and each signed
   artifact set passed evidence verification for the expected builder workflow and source commit.
   Preserve the reports and `kunlun-jsc-*` artifacts outside Actions storage before their retention
   period expires.
3. **Publish only verified bytes.** Transfer each target's archive, SPDX SBOM, signed provenance,
   `SHA256SUMS`, and independent-rebuild report to the maintainer-approved durable release storage.
   Verify the transferred bytes and record the storage URL and SHA-256 digests. Do not publish a
   candidate or update the manifest from a normal CI artifact.
4. **Record publication in a reviewed PR.** Change all three evidence digests (archive, SBOM, and
   provenance) for each successfully published target in one focused update to
   `distribution/jsc/manifest.json`, changing its status from `planned` to `published` only after
   the files are durably available. Run:

   ```bash
   cargo xtask jsc-manifest validate
   ```

   Include the immutable tag and commit, M2 run URL, evidence summary, storage location, and
   published digests in the PR. Merge the manifest update through the normal review and merge-queue
   process. Never edit the manifest directly on `main`.
5. **Complete the release record.** Link the merged manifest PR and durable asset location to the
   release record. Keep the tag, run URL, full source SHA, manifest digest, archive/SBOM/provenance
   digests, and independent-rebuild reports together so a consumer can reproduce the trust decision.

The artifact builders upload temporary evidence only. They do not update `planned` targets, publish
a GitHub Release, or copy files to durable storage. Publication remains an explicit maintainer action
after signed evidence and the destination bytes have been reviewed. See the
[artifact workflow policy](./jsc-distribution.md#when-to-run-the-artifact-workflows) and
[M2 exit-gate procedure](./m2-exit-gate.md) for the exact evidence and verification boundaries.

## Product release boundaries

An M2 artifact-validation pass qualifies the pinned JSC boundary; it does not by itself qualify a
Kunlun Runtime product release. Use the capability labels and exit gates in the
[roadmap](../ROADMAP.md). In particular, the `0.1` Runtime preview includes M3 application
compatibility, which remains a separate acceptance gate. The `kunlun-runtime` crate is not published
to crates.io.

The checked-in distribution manifest is the source of truth for supported JSC artifact targets. It
currently covers macOS arm64/x64 and Linux glibc arm64/x64; Windows is explicitly gated on a
supportable WebKit/JSC build and release story. Do not advertise or add `x86_64-pc-windows-msvc` as a
Runtime release target until that engine path and its artifact, test, and signed-evidence gates have
been qualified.

If a published artifact is defective, do not move its tag or overwrite the asset. Preserve the
original evidence, report the affected digest and impact, and issue a separately reviewed corrective
release with a new immutable tag.
