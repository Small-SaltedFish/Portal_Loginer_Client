# Windows transport patch

The workspace pins hyper-util 0.1.21 in Cargo.lock and vendors its original
MIT license with the source. Local changes are confined to:

- src/client/legacy/connect/http.rs: for a configured Windows IPv4 source,
  resolve its current adapter index via GetIpAddrTable, set IP_UNICAST_IF
  in network byte order, then bind the socket. Missing interfaces fail
  without falling back to the default route.
- src/client/legacy/client.rs: silence an unused timer binding when HTTP/2
  support is disabled.

Reason: binding only the source IPv4 address did not reliably pin the actual
outlet in the dual-adapter campus test. All reqwest HTTP connections, including
TLS, use the selected adapter while preserving the destination hostname.
UDP DNS binding is implemented in portal-core/src/bound_dns.rs.

The original crate source is retained to make the small patch reviewable.
