# clamd-client, WIP

Rust async tokio client for clamd. Works with a tcp socket or with the unix socket. At the moment it will open a
new socket for each command. Work in progress.

## Notes
### To test you have to have a running clamd instance on your machine. Easiest would be to use docker:
```
docker run -p 3310:3310  -v /run/clamav/:/run/clamav/  clamav/clamav:unstable
```

## TODOS
- Implement missing clamd functionality
- Implement keepalive tcp connection
