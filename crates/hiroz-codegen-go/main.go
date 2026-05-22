package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
)

func main() {
	inputFile := flag.String("input", "", "Input JSON manifest file (or - for stdin)")
	outputDir := flag.String("output", ".", "Output directory for generated Go code")
	packagePrefix := flag.String("prefix", "hiroz", "Go package prefix")
	flag.Parse()

	var data []byte
	var err error

	if *inputFile == "" || *inputFile == "-" {
		data, err = io.ReadAll(os.Stdin)
	} else {
		data, err = os.ReadFile(*inputFile)
	}
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to read input: %v\n", err)
		os.Exit(1)
	}

	var manifest CodegenManifest
	if err := json.Unmarshal(data, &manifest); err != nil {
		fmt.Fprintf(os.Stderr, "Failed to parse JSON: %v\n", err)
		os.Exit(1)
	}

	if manifest.Version != ExpectedVersion {
		fmt.Fprintf(os.Stderr, "Version mismatch: got %d, expected %d\n",
			manifest.Version, ExpectedVersion)
		os.Exit(1)
	}

	// Generate code for each message
	for _, msg := range manifest.Messages {
		if err := generateMessage(msg, *outputDir, *packagePrefix); err != nil {
			fmt.Fprintf(os.Stderr, "Failed to generate %s: %v\n", msg.FullName, err)
			os.Exit(1)
		}
		fmt.Printf("Generated: %s\n", msg.FullName)
	}

	// Generate code for each service
	for _, srv := range manifest.Services {
		if err := generateService(srv, *outputDir, *packagePrefix); err != nil {
			fmt.Fprintf(os.Stderr, "Failed to generate service %s: %v\n", srv.FullName, err)
			os.Exit(1)
		}
		fmt.Printf("Generated service: %s\n", srv.FullName)
	}

	// Generate code for each action
	for _, action := range manifest.Actions {
		if err := generateAction(action, *outputDir, *packagePrefix); err != nil {
			fmt.Fprintf(os.Stderr, "Failed to generate action %s: %v\n", action.FullName, err)
			os.Exit(1)
		}
		fmt.Printf("Generated action: %s\n", action.FullName)
	}

	fmt.Printf("Generation complete: %d messages, %d services, %d actions\n",
		len(manifest.Messages), len(manifest.Services), len(manifest.Actions))
}

func generateMessage(msg MessageDefinition, baseDir, prefix string) error {
	pkgDir := filepath.Join(baseDir, sanitizePackageName(msg.Package))
	if err := os.MkdirAll(pkgDir, 0755); err != nil {
		return err
	}

	code, err := GenerateGoMessage(msg, prefix)
	if err != nil {
		return err
	}

	filename := filepath.Join(pkgDir, strings.ToLower(msg.Name)+".go")
	return os.WriteFile(filename, code, 0644)
}

func generateAction(action ActionDefinition, baseDir, prefix string) error {
	pkgDir := filepath.Join(baseDir, sanitizePackageName(action.Package))
	if err := os.MkdirAll(pkgDir, 0755); err != nil {
		return err
	}

	code, err := GenerateGoAction(action, prefix)
	if err != nil {
		return err
	}

	filename := filepath.Join(pkgDir, "action_"+strings.ToLower(action.Name)+".go")
	return os.WriteFile(filename, code, 0644)
}

func generateService(srv ServiceDefinition, baseDir, prefix string) error {
	// Services go in the same directory as messages (same Go package)
	pkgDir := filepath.Join(baseDir, sanitizePackageName(srv.Package))
	if err := os.MkdirAll(pkgDir, 0755); err != nil {
		return err
	}

	code, err := GenerateGoService(srv, prefix)
	if err != nil {
		return err
	}

	filename := filepath.Join(pkgDir, "srv_"+strings.ToLower(srv.Name)+".go")
	return os.WriteFile(filename, code, 0644)
}

func sanitizePackageName(name string) string {
	return strings.ReplaceAll(name, "-", "_")
}

// ExampleGenerateGoMessage is a helper function to verify code generation
func ExampleGenerateGoMessage() ([]byte, error) {
	msg := MessageDefinition{
		Package:  "std_msgs",
		Name:     "String",
		FullName: "std_msgs/String",
		TypeHash: "RIHS01_1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
		Fields: []FieldDefinition{
			{
				Name:      "data",
				FieldType: FieldType{Kind: "String"},
				IsArray:   false,
			},
		},
		Constants: []ConstantDefinition{},
	}

	return GenerateGoMessage(msg, "hiroz")
}
