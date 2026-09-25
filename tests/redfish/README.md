These certificates and the server private key are public, disposable test fixtures.
They are valid only for localhost/127.0.0.1 and are never used by the application,
deployment, or a real BMC. Do not install this CA as a system trust root.

Run `python tests/redfish/run.py` from the repository root. The Python HTTPS server
binds to a random loopback port; the Rust provider trusts its CA only in this test.
