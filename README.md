# a download tool for gelbooru.com

Used daily by me for its intended purpose. For pickier users `gallery-dl` must be the better choice.

## Examples

```sh
# Download 'touhou' tag to /home/user/gelbooru.
# Note: There's 825k images tagged "touhou". All of the links likely won't fit into your ram.
gelbooru-scraper -o "/home/user/gelbooru" touhou

# Download 'touhou' tag and metadata about posts to the current directory.
gelbooru-scraper -j -o . touhou

# Download all general images of Kagerou or Nitori with a score of 20 or higher using HTTP/2.
gelbooru-scraper -2 -o . {imaizumi_kagerou ~ kawashiro_nitori} score:>=20 rating:g
```

## Usage

Here's the help message as of 0.3.1:

```
Usage: gelbooru-scraper.exe [OPTIONS] -o <OUTPUT_DIR> [TAGS]...

Arguments:
  [TAGS]...  Whitespace-separated list of tags to search for.

Options:
  -y
          
  -o <OUTPUT_DIR>
          
      --api-key <API_KEY>
          Optional api key. Has to be specified with user_id. Can be found at https://gelbooru.com/index.php?page=account&s=options
      --user-id <USER_ID>
          Optional user id. Has to be specified with api_key. Can be found at https://gelbooru.com/index.php?page=account&s=options
  -j, --write-json [<WRITE_JSON>]
          Write post metadata to a JSON file. If no path is specified, writes to <OUTPUT_DIR>/posts.json.
          Path is relative to <OUTPUT_DIR>.
          If path is '-', writes to stderr.
  -J, --write-pretty-json [<WRITE_PRETTY_JSON>]
          Makes the metadata JSON human-readable. Implies '--write-json'.
  -1, --http1
          Use HTTP/1.1.
  -2, --http2
          Use HTTP/2.
  -3, --http3
          Use HTTP/3. Enabled by default.
  -h, --help
          Print help
  -V, --version
          Print version
```