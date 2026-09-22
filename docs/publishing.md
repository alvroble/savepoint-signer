# Publishing Savepoint Signer

The public repository is [alvroble/savepoint-signer](https://github.com/alvroble/savepoint-signer).
The first source snapshot is prepared in a separate local Git repository so
its public history starts with the maintained integration and does not expose
removed experimental approaches through old commits. It retains explicit
credits to the Croco hardware/firmware and pret/pokecrystal projects.

## First signed commit

Review the staged snapshot and sign its initial commit locally:

```sh
git status --short
git diff --cached --check
git diff --cached --stat
git commit -S -m "Publish Savepoint Signer prototype"
git push -u origin main
```

The upstream Croco firmware repository is **not** the publish destination.
The new repository's `origin` must point to
`https://github.com/alvroble/savepoint-signer.git` before pushing. If HTTPS
Git authentication is unavailable, refresh `gh auth` or use your own configured
SSH remote; do not push to the old `shilga` remote.

## Release gate

Publishing source does not qualify the signer for real funds. Before tagging a
binary release, run the local checks in [testing](testing.md), confirm ordinary
game SAVE/CONTINUE and seed recovery/lock/return on the physical cartridge,
and exercise PSBT import/review/sign/export with a public synthetic fixture.
The consolidated firmware and latest UI have not yet had that complete
physical smoke test. Review the first GitHub Actions run as well.

After those checks, rebuild from the signed clean commit and make a draft
prerelease. The package manifest must say `"dirty": false` and name the same
commit as the tag. The archive includes source, screenshots, the Crystal source
patch and the RP2350 ELF; it excludes the full game ROM, saves, SD contents and
user PSBTs. The tag workflow creates a draft prerelease for human review.
