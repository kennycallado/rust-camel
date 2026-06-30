# Merge to main prompt

Paste the block below; replace `rc-XXXX` with this feature's actual bd issue id before sending. If a beads-close hook is active (B1.4), it will close automatically — otherwise the agent closes it via `bd close` after merge.

---

## MERGE TO MAIN

Skill: `finishing-a-development-branch`, option 1 (squash merge into main).
Single commit: title + body following existing commit pattern (`caveman-commit`).
Update implementation doc + postmortem if applicable.
Clean up branch + worktree.
Verify README/s are up to date.
Update `docs/roadmap.md` and `docs/status.md` (don't track docs/ in git).
Move related `docs/superpowers/` files to `docs/superpowers/archived/`.
Check `./scripts/publish-crates.sh` and `camel-cli` features.
Close bd issue: `rc-XXXX` (replace with this feature's bd issue id; skip if already closed by hook).
