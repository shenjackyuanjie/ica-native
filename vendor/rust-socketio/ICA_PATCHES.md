# Local rust-socketio fixes

The vendored dependency is based on the previously used
`nextdata-tech/rust-socketio` branch.

`socketio/src/asynchronous/client/client.rs` contains two local correctness fixes:

- allocate monotonically increasing Socket.IO ACK IDs instead of random IDs in
  `0..999`, which collided during large batches of group-member requests;
- remove exactly one matching ACK while holding the lock, then invoke its
  callback after releasing the lock. This avoids index-shift panics and avoids
  holding the ACK list lock across user callbacks.
