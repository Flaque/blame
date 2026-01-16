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

## Use with GitHub PRs

Get GitHub usernames for PR reviewers:

```bash
# Get top contributor's GitHub username
blame --gh --only-name src/

# Use directly with gh pr create
gh pr create --reviewer $(blame --gh --only-name src/)

# Get all contributors
blame --gh --only-name -v src/
```
