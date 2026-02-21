package main

import (
	"os"

	"github.com/ggonzalez94/defi-cli/internal/app"
)

func main() {
	runner := app.NewRunner()
	os.Exit(runner.Run(os.Args[1:]))
}
