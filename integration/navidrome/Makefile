DOCKER_COMPOSE ?= docker-compose
CARGO ?= cargo
COMPOSE = $(DOCKER_COMPOSE) -p libresonic-navidrome -f docker-compose.yml
NAVIDROME_PORT ?= 4535
export NAVIDROME_PORT

.PHONY: library up smoke rust-smoke down test test-rust

library:
	python3 prepare.py

up: library
	$(COMPOSE) up -d --wait

smoke:
	python3 smoke.py --url "http://127.0.0.1:$(NAVIDROME_PORT)"

rust-smoke: smoke
	cd ../.. && LIBRESONIC_TEST_URL="http://127.0.0.1:$(NAVIDROME_PORT)" $(CARGO) test --locked --test navidrome -- --ignored --nocapture

down:
	$(COMPOSE) down --volumes

# Always remove this fixture's containers/volume, including on smoke failure.
test:
	@set -e; trap '$(MAKE) down' EXIT; $(MAKE) up; $(MAKE) smoke

test-rust:
	@set -e; trap '$(MAKE) down' EXIT; $(MAKE) up; $(MAKE) rust-smoke
