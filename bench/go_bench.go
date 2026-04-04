// Benchmark CLI for Go JSONata.
// Usage: go_bench -expr 'Account.Name' -data '{"Account":{"Name":"Firefly"}}' [-n 1000]
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"os"

	"github.com/recolabs/gnata"
)

func main() {
	expr := flag.String("expr", "", "JSONata expression")
	data := flag.String("data", "{}", "JSON input")
	datafile := flag.String("datafile", "", "JSON input file (overrides -data)")
	n := flag.Int("n", 1, "iterations (for hot-loop benchmarking)")
	flag.Parse()

	if *expr == "" {
		fmt.Fprintln(os.Stderr, "usage: go_bench -expr EXPR [-data JSON | -datafile FILE] [-n ITERS]")
		os.Exit(1)
	}

	compiled, err := gnata.Compile(*expr)
	if err != nil {
		fmt.Fprintf(os.Stderr, "compile error: %v\n", err)
		os.Exit(1)
	}

	jsonStr := *data
	if *datafile != "" {
		b, err := os.ReadFile(*datafile)
		if err != nil {
			fmt.Fprintf(os.Stderr, "read file error: %v\n", err)
			os.Exit(1)
		}
		jsonStr = string(b)
	}

	var input any
	if err := json.Unmarshal([]byte(jsonStr), &input); err != nil {
		fmt.Fprintf(os.Stderr, "json error: %v\n", err)
		os.Exit(1)
	}

	ctx := context.Background()
	var result any
	for range *n {
		result, err = compiled.Eval(ctx, input)
		if err != nil {
			fmt.Fprintf(os.Stderr, "eval error: %v\n", err)
			os.Exit(1)
		}
	}

	out, _ := json.Marshal(result)
	fmt.Println(string(out))
}
