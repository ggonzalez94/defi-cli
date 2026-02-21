.PHONY: build test test-race vet fmt run release-check release-snapshot

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

release-check:
	goreleaser check

release-snapshot:
	goreleaser release --snapshot --clean
