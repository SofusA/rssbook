# rssbook
Rss feeds to e-book builder.

## Installation
Install with homebrew:
```
  brew trust sofusa/tab
  brew install sofusa/rssbook
```

## Usage
1. Create a `rssbook.toml`. See `[rssbook](/blob/main/example_rssbook.toml)`.
1. Run `rssbook` in same directory.

## Flags
```
      --config <CONFIG>                        [default: ./rssbook.toml]
  -s, --select                                 Select read articles, which will not be bundled
  -u, --upload-crosspoint <UPLOAD_CROSSPOINT>  URL for `CrossPoint` device
  -h, --help                                   Print help
  ```
