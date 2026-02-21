.PHONY: build test test-race vet fmt run

build:
	go build -o defi ./cmd/defi

test:
	go test ./...

test-race:
	go test -race ./...

vet:
	go vet ./...

fmt:
	gofmt -w $$(find . -name '*.go' -type f)

run:
	go run ./cmd/defi $(ARGS)
