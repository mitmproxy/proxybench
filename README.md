# proxybench

A minimal proxy server benchmarking tool.

Usage: Run the following two commands in two different command line windows.
```shell
mitmdump --ssl-insecure -q
cargo criterion
```

See `benches/proxybench.rs` for configuration options.