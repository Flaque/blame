# blame

Find who's responsible for a file or folder using git blame.

```
blame src/          # top contributor for directory
blame "**/*.rs" -v  # all contributors for glob pattern
```

## Install

```
git clone https://github.com/flaque/blame /tmp/blame && cargo install --path /tmp/blame
```
