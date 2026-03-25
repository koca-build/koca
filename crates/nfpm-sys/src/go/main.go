package main

// #include <stdint.h>
// #include <stdlib.h>
//
// const uint8_t STATUS_SUCCESS = 0;
// const uint8_t STATUS_JSON = 1;
// const uint8_t STATUS_OUTPUT_FILE = 2;
// const uint8_t STATUS_PKG_CREATION = 3;
import "C"
import (
	"encoding/json"
	"os"

	"github.com/goreleaser/nfpm/v2"
	"github.com/goreleaser/nfpm/v2/deb"
	"github.com/goreleaser/nfpm/v2/files"
	"github.com/goreleaser/nfpm/v2/rpm"
)

// NfpmFileInfo represents nfpm file info
type NfpmFileInfo struct {
	Mode uint32 `json:"mode"`
}

// NfpmFile represents nfpm package file mappings
type NfpmFile struct {
	Src      string       `json:"src"`
	Dst      string       `json:"dst"`
	FileInfo NfpmFileInfo `json:"file_info"`
}

// NfpmConfig represents an nfpm config
type NfpmConfig struct {
	Name        string     `json:"name"`
	Arch        string     `json:"arch"`
	Platform    string     `json:"platform"`
	Epoch       *uint32    `json:"epoch,omitempty"`
	Version     string     `json:"version"`
	Release     *uint32    `json:"release,omitempty"`
	Maintainer  string     `json:"maintainer"`
	Description string     `json:"description"`
	License     string     `json:"license"`
	Contents    []NfpmFile `json:"contents"`
}

func main() {}

func charToString(cstr *C.char) string {
	if cstr == nil {
		panic("Received nil C string")
	}
	return C.GoString(cstr)
}

//export runBundle
func runBundle(rawOutputFile *C.char, rawFormat *C.char, rawInputJson *C.char) uint8 {
	outputFile := charToString(rawOutputFile)
	format := charToString(rawFormat)
	inputJsonStr := charToString(rawInputJson)

	var nfpmConfig NfpmConfig
	if err := json.Unmarshal([]byte(inputJsonStr), &nfpmConfig); err != nil {
		return C.STATUS_JSON
	}

	var pkgFiles files.Contents

	for _, file := range nfpmConfig.Contents {
		pkgFiles = append(pkgFiles, &files.Content{
			Source:      file.Src,
			Destination: file.Dst,
			FileInfo: &files.ContentFileInfo{
				Mode: os.FileMode(file.FileInfo.Mode),
			},
		})
	}

	pkgInfo := nfpm.Info{
		Name:        nfpmConfig.Name,
		Version:     nfpmConfig.Version,
		Description: nfpmConfig.Description,
		Platform:    nfpmConfig.Platform,
		Section:     "default",
		Arch:        nfpmConfig.Arch,
		Maintainer:  nfpmConfig.Maintainer,
		Overridables: nfpm.Overridables{
			Contents: pkgFiles,
		},
	}

	outputBin, err := os.Create(outputFile)
	if err != nil {
		return C.STATUS_OUTPUT_FILE
	}
	defer outputBin.Close()

	var pkgError error
	switch format {
	case "deb":
		pkgError = deb.Default.Package(&pkgInfo, outputBin)
	case "rpm":
		pkgError = rpm.DefaultRPM.Package(&pkgInfo, outputBin)
	default:
		panic("Unsupported package format: " + format)
	}

	if pkgError != nil {
		return C.STATUS_PKG_CREATION
	}

	return C.STATUS_SUCCESS
}
