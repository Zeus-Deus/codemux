# Disposable native TLS fixture

Public test CA and server certificate for `addon-http.invalid`, plus the server's
**non-secret test key**. Never use this key or CA outside these tests. The private
CA signing key is not committed. These certificates expire September 2046.

The test-only network seam routes a vetted synthetic public DNS answer to a
loopback TLS listener. Production still resolves public DNS and pins those actual
addresses. Only the selected fixture trusts this CA; system trust is untouched.
The tests exercise the real HTTP/TLS client, including rejection with no added
CA, redirects, streaming body ceilings and cancellation. Fixture state is removed
from non-test compilation.
