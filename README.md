# proxybench

A minimal proxy server benchmarking tool.

Usage: Run the following four commands in four different command line windows.
```shell
mitmdump -q --mode reverse:http://127.0.0.1:3000 -p 8000  # Plain HTTP/1.1
mitmdump -q --mode reverse:https://127.0.0.1:3001 -k -p 8001  # HTTP/1.1 over TLS
mitmdump -q --mode reverse:https://127.0.0.1:3002 -k -p 8002  # HTTP/2.0 over TLS
cargo criterion
```
