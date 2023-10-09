# Tools
Install needed tools (for example using [asdf version manager](https://asdf-vm.com/)):

ruby 3.0.2
nodejs 14.20.0
# Setup
Run:
```bash
bin/setup
```
## Database
Run db migrations:
```bash
bundle exec rails db:migrate
```
# Run the app
bundle assets with webpack
```bash
bin/webpack
```
or run webpack server
```bash
bin/webpack-dev-server
```
run the server:
```bash
bundle exec rails s
```
run redis server (for bots, metrics and fees service):
```bash
redis-server
```

To inspect redis you can use redis-cli and command line
```bash
redis-cli
```
Inspect activities in your Redis server in real-time. Run the following command in the Redis CLI:
```bash
MONITOR
```

or use GUI, for example [redis commander](https://github.com/joeferner/redis-commander)

# Additional info:

- You can run test automatically using guard gem:
  ```bash
  bundle exec guard -c
  ```
