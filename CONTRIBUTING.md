# Contributing

## What's planned

Everything that's planned or being worked on is on the **[project board](https://github.com/users/cweeperde/projects/1)**. Anyone can look at it; only maintainers can edit it.

## Found a bug or have an idea?

[Open an issue](https://github.com/cweeperde/scribble2fix/issues/new/choose) and pick a template. It shows up on the board automatically. Something that could expose private data? Please [report it privately](SECURITY.md) instead.

## For maintainers

You need the [GitHub CLI](https://cli.github.com/) (`gh`), signed in with `gh auth login`.

**Add a todo**

```
gh issue create -l todo -t "Short description of the task"
```

It lands in **Todo** on the board.

**Open the board**

```
gh project view 1 --owner cweeperde --web
```

**Close a todo with a commit**

Put `Fixes #12` (with the issue's number) anywhere in the commit message:

```
git commit -m "Ignore the hotkey while OBS is focused" -m "Fixes #12"
```

When the commit reaches `main`, the issue closes and its card moves to **Done**. For several issues, repeat the word: `Fixes #12, fixes #15`. Just `#12` links the commit without closing the issue.
