# Publishing the reviewed contribution

The prepared branch is `contrib/xpro2-research`. Its parent is the public
upstream commit recorded in `data/provenance.generated.json`; it does not descend from
private laboratory commits. The existing laboratory `main` is unchanged.

## Before uploading

```sh
python3 research/xpro2/audit-public.py
python3 -m unittest discover -s research/xpro2 -p 'test_*.py'
git diff --stat origin/main...contrib/xpro2-research
git log --format=fuller origin/main..contrib/xpro2-research
```

Review the branch diff, commit author identity and all files under
`research/xpro2`. The local contribution commit uses a generic research identity;
replace it with an intentionally public identity if personal attribution is
wanted. Changing an author should never accidentally expose a private email.
The automated scan recognizes common credentials and personal paths; it is not
a substitute for reviewing source and provenance.

## Use a fork, not the upstream main branch

Create a fork of `justjake/fujimod` under the intended GitHub account, then add
that fork as a separate remote. Replace YOUR_ACCOUNT with the chosen public
account name. Do not put credentials in a remote URL.

```sh
git remote add xpro2-fork https://github.com/YOUR_ACCOUNT/fujimod.git
git push xpro2-fork contrib/xpro2-research
```

Open a **draft pull request** from that branch to `justjake/fujimod:main`, using
[PR-DRAFT.md](PR-DRAFT.md). The first review is for the reproducible research
handoff. Actual current-runtime code ports can follow in smaller, tested PRs.

Push only the named contribution branch. Do not use `push --all`, mirror the
laboratory repository, or upload its history, raw captures or generated inputs.
The public source capsule deliberately excludes those materials.

## Optional Git bundle

The delivery bundle contains only the contribution branch beyond its public
base. In a FujiMod checkout with the base available:

```sh
git fetch /path/to/xpro2-contribution.bundle \
  refs/heads/contrib/xpro2-research:refs/heads/contrib/xpro2-research
git switch contrib/xpro2-research
```

A bundle is a local transfer artifact, not evidence that anything was uploaded
to GitHub. No PR, push or public message is created by the preparation tools.
