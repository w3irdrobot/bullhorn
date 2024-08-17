# Bullhorn

Simple nostr notification service.

## Example Config

The location of the config is expected to be in the location for your machine as defined in the
[`dirs` crate](https://docs.rs/dirs/latest/dirs/). On Linux, that will be `$HOME/.config/bullrun/config.toml`.
The config is a TOML file. An example is below.

```toml
# The npub to monitor and notify of events on
npub = "npub1kmgpttf3hzmpnfa9jrpu99tqr8x865r2m7mkwwcvfs7pazm6dnvq5c97vh"

# Special npubs that have live events you want to be notified about
event_npubs = [
  # RHR
  "npub10uthwp4ddc9w5adfuv69m8la4enkwma07fymuetmt93htcww6wgs55xdlq",
]
```

## Development

Ensure Rust and Cargo are installed. The easiey way to do that is using [rustup](https://rustup.rs/). Then run the development server.

```shell
cargo run
```

## Support

PRs are more than welcome!

Feeling generous? Leave me a tip! ⚡️w3irdrobot@vlt.ge.

Think I'm an asshole but still want to tip? Please donate [to OpenSats](https://opensats.org/).

Want to tell me how you feel? Hit me up [on Nostr](https://njump.me/rob@w3ird.tech).

## License

Distributed under the AGPLv3 License. See [LICENSE.txt](./LICENSE.txt) for more information.
