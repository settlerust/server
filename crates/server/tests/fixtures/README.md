# Authentication test keys

`jwt-test-private.pem` and `jwt-test-public.pem` are a public RSA key pair used
only by `tests/managed_auth.rs` to sign and verify synthetic game grants. They
provide reproducible signature tests without an external identity service.

Treat the private key as public test data. Never configure it as a deployed
issuer's signing key or reuse it for another purpose. The native server accepts
an operator-provided verification key; it does not load this fixture at runtime.

The secret scanner allows the private-key rule for this exact fixture path.
Other rules and paths remain subject to the default scan.
