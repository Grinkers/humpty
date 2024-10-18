# humpty
humpty is fast compiling low-latency HTTP/1.1 web server, with support for static content and WebSockets.
humpty is currently under active development.
While it does work, large amounts of breaking changes are expected.

## Goals
- Simplicity (simple source, but also simple to use)
- Low-latency
- Fast compile times
- Safety (unsafe_code is denied)

## Non-goals
- [C10k problem](https://en.wikipedia.org/wiki/C10k_problem)
- async

  If you need to use async api's, something like
  [pollster](https://github.com/zesterer/pollster) is recommended.

## Special Thanks
- [Humphrey](https://github.com/w-henderson/Humphrey)
