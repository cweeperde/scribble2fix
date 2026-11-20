# Contributing to Scribble2Fix

Thanks for helping! Bug reports and ideas are welcome as [issues](https://github.com/cweeperde/scribble2fix/issues/new/choose). For code, please read this page first: Scribble2Fix is source-available, not open source, so contributions need the agreement below.

## How to contribute

1. For anything bigger than a small fix, open an issue first so we can agree on the approach.
2. Fork the repository (or, if you have write access, create a branch). Never push to `main` or create `v*` tags: releases go out to every user through auto-update.
3. Make your change and test it on your platform. Keep pull requests focused on one thing.
4. Open a pull request against `main` and tick the agreement in the template.

The maintainer reviews, merges and releases.

## Building locally

You need Rust (stable) and Node.js 22.

```sh
cd app
npx @tauri-apps/cli@2 dev                      # run it
npx @tauri-apps/cli@2 build --config '{"bundle":{"createUpdaterArtifacts":false}}'
```

The `--config` part skips the signed update files, which need the maintainer's private key. On Linux, install the packages listed in `.github/workflows/build.yml` first.

Note: running a development build registers its own path as the `scribble2fix` MCP server in your connected AI tools. Start the installed app again afterwards to restore it.

## Contributor agreement

By opening a pull request or otherwise submitting a contribution (code, documentation, images or other material) to this repository, you agree to the following:

1. **Your right to contribute.** The contribution is your own original work, or you have the right to submit it under these terms, and it doesn't knowingly infringe anyone else's rights.
2. **Rights for the maintainer.** You transfer to cweeper, the maintainer of Scribble2Fix, all rights in your contribution to the extent the law allows. Where a transfer isn't possible (for example the author's copyright under German law), you grant cweeper an exclusive, worldwide, perpetual, irrevocable, royalty-free and transferable license to use, copy, change, combine with other work, publish, distribute and sublicense the contribution, in any form and under any license, including commercial and closed-source licenses.
3. **Your own use.** cweeper grants you back a non-exclusive license to use your own contribution in your other work, independent of Scribble2Fix.
4. **No obligation.** cweeper doesn't have to use or merge your contribution and may change or remove it at any time.
5. **No payment or warranty.** You don't receive payment for the contribution, and you provide it as is, without warranty.
6. **Moral rights.** As far as the law allows, you agree not to assert moral rights (such as the right to be named or to object to changes) against cweeper or anyone licensed by them. You may still be credited in the commit history.

If you can't agree to this, please open an issue describing the change instead of sending code.
