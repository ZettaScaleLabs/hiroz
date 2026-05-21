package main

import (
	"bytes"
	"fmt"
	"go/format"
	"unicode"
)

type CodeBuilder struct {
	buf        bytes.Buffer
	indent     int
	currentPkg string // current Go package being generated
}

func (b *CodeBuilder) P(format string, args ...interface{}) {
	for i := 0; i < b.indent; i++ {
		b.buf.WriteString("\t")
	}
	fmt.Fprintf(&b.buf, format, args...)
	b.buf.WriteString("\n")
}

func (b *CodeBuilder) In()  { b.indent++ }
func (b *CodeBuilder) Out() { b.indent-- }

func (b *CodeBuilder) Bytes() ([]byte, error) {
	return format.Source(b.buf.Bytes())
}

// cdrAlignment returns the CDR-LE alignment (in bytes) required before
// reading or writing a value of the given primitive kind. The alignment
// applies to the offset measured from the start of the encapsulation body.
func cdrAlignment(kind string) int {
	switch kind {
	case "Bool", "Int8", "UInt8":
		return 1
	case "Int16", "UInt16":
		return 2
	case "Int32", "UInt32", "Float32", "String", "Time", "Duration":
		return 4
	case "Int64", "UInt64", "Float64":
		return 8
	default:
		return 1
	}
}

// emitPackAlign emits inline code that pads `buf` and advances `offset` to
// the next align-byte boundary. No-op for align <= 1.
func emitPackAlign(g *CodeBuilder, align int) {
	if align <= 1 {
		return
	}
	g.P("if pad := (%d - offset%%%d) %% %d; pad > 0 {", align, align, align)
	g.In()
	g.P("buf = append(buf, make([]byte, pad)...)")
	g.P("offset += pad")
	g.Out()
	g.P("}")
}

// emitUnpackAlign emits inline code that advances `offset` to the next
// align-byte boundary. No-op for align <= 1.
func emitUnpackAlign(g *CodeBuilder, align int) {
	if align <= 1 {
		return
	}
	g.P("offset += (%d - offset%%%d) %% %d", align, align, align)
}

// fieldsHaveNested reports whether any field requires a nested unpack call
// that would assign to `err`. Used to decide whether to declare `var err
// error` at the top of the unpack body.
func fieldsHaveNested(fields []FieldDefinition) bool {
	for _, f := range fields {
		switch f.FieldType.Kind {
		case "Custom", "Time", "Duration":
			return true
		}
	}
	return false
}

// emitMessageCodec emits SerializeCDR, PackToRawAt, DeserializeCDR, and
// UnpackFromRawAt methods for a struct named typeName. Used for plain
// messages, service request/response types, and action sub-message types.
func emitMessageCodec(g *CodeBuilder, typeName string, fields []FieldDefinition) {
	// SerializeCDR — public entry, produces a CDR-LE encapsulated buffer.
	g.P("// SerializeCDR serializes the message to CDR format")
	g.P("func (m *%s) SerializeCDR() ([]byte, error) {", typeName)
	g.In()
	g.P("buf := make([]byte, 4, 256)")
	g.P("buf[0], buf[1], buf[2], buf[3] = 0x00, 0x01, 0x00, 0x00 // CDR_LE encapsulation header")
	g.P("buf, _ = m.PackToRawAt(buf, 0) // top-level call: final offset equals body length, not needed here")
	g.P("return buf, nil")
	g.Out()
	g.P("}")
	g.P("")

	// PackToRawAt — internal body emitter. Exported so cross-package nested
	// types can call it directly with the outer message's current offset.
	g.P("// PackToRawAt appends this message's CDR-encoded body bytes to buf.")
	g.P("// offset is the absolute body position (relative to the encapsulation")
	g.P("// header end) where the next byte will land; the function uses it for")
	g.P("// alignment padding and returns the updated buf and new offset.")
	g.P("func (m *%s) PackToRawAt(buf []byte, offset int) ([]byte, int) {", typeName)
	g.In()
	for _, field := range fields {
		generatePackField(g, field)
	}
	g.P("return buf, offset")
	g.Out()
	g.P("}")
	g.P("")

	// DeserializeCDR — public entry, accepts a CDR-LE encapsulated buffer.
	g.P("// DeserializeCDR deserializes CDR data into the message")
	g.P("func (m *%s) DeserializeCDR(data []byte) error {", typeName)
	g.In()
	g.P("if len(data) < 4 { return fmt.Errorf(\"CDR data too short: need 4-byte encapsulation header\") }")
	g.P("_, err := m.UnpackFromRawAt(data[4:], 0)")
	g.P("return err")
	g.Out()
	g.P("}")
	g.P("")

	// UnpackFromRawAt — internal body parser. Exported for cross-package use.
	g.P("// UnpackFromRawAt reads from data starting at the given absolute body")
	g.P("// offset. Returns the new offset (after this message's bytes) so the")
	g.P("// caller can continue parsing subsequent fields with correct alignment.")
	g.P("func (m *%s) UnpackFromRawAt(data []byte, offset int) (int, error) {", typeName)
	g.In()
	if fieldsHaveNested(fields) {
		g.P("var err error")
	}
	for _, field := range fields {
		generateUnpackField(g, field)
	}
	g.P("return offset, nil")
	g.Out()
	g.P("}")
	g.P("")
}

func GenerateGoMessage(msg MessageDefinition, prefix string) ([]byte, error) {
	currentPkg := sanitizePackageName(msg.Package)
	g := &CodeBuilder{currentPkg: currentPkg}

	// Analyze imports needed
	needsMath := false
	needsBinary := false
	crossPkgImports := map[string]bool{}
	for _, field := range msg.Fields {
		if field.FieldType.Kind == "Float32" || field.FieldType.Kind == "Float64" {
			needsMath = true
		}
		if fieldNeedsBinary(field) {
			needsBinary = true
		}
		if field.FieldType.Kind == "Custom" && field.FieldType.Package != nil {
			pkg := sanitizePackageName(*field.FieldType.Package)
			if pkg != currentPkg {
				crossPkgImports[pkg] = true
			}
		}
		if field.FieldType.Kind == "Time" || field.FieldType.Kind == "Duration" {
			if currentPkg != "builtin_interfaces" {
				crossPkgImports["builtin_interfaces"] = true
				needsBinary = true
			}
		}
	}

	// Package declaration
	g.P("package %s", sanitizePackageName(msg.Package))
	g.P("")

	// Imports
	g.P("import (")
	g.In()
	if needsBinary {
		g.P(`"encoding/binary"`)
	}
	g.P(`"fmt"`)
	if needsMath {
		g.P(`"math"`)
	}
	for pkg := range crossPkgImports {
		g.P(`"%s/generated/%s"`, prefix, pkg)
	}
	g.Out()
	g.P(")")
	g.P("")

	// Generate struct
	generateStructInPkg(g, msg, sanitizePackageName(msg.Package))

	// Generate constants
	generateConstants(g, msg)

	// Generate TypeName method
	generateTypeNameMethod(g, msg)

	// Generate TypeHash method
	generateTypeHashMethod(g, msg)

	// Generate SerializeCDR / DeserializeCDR / PackToRawAt / UnpackFromRawAt
	emitMessageCodec(g, msg.Name, msg.Fields)

	return g.Bytes()
}

func generateStruct(g *CodeBuilder, msg MessageDefinition) {
	generateStructInPkg(g, msg, "")
}

func generateStructInPkg(g *CodeBuilder, msg MessageDefinition, currentPkg string) {
	g.P("// %s is a ROS 2 message type", msg.Name)
	g.P("// Full name: %s", msg.FullName)
	g.P("type %s struct {", msg.Name)
	g.In()

	for _, field := range msg.Fields {
		goType := fieldToGoTypeInPkg(field, currentPkg)
		g.P("%s %s", capitalize(field.Name), goType)
	}

	g.Out()
	g.P("}")
	g.P("")
}

func generateConstants(g *CodeBuilder, msg MessageDefinition) {
	// Use DDS-qualified type name so Zenoh key expressions match rmw_zenoh_cpp and ros-z Rust.
	ddsTypeName := fmt.Sprintf("%s::msg::dds_::%s_", msg.Package, msg.Name)
	g.P("const (")
	g.In()
	g.P("%s_TypeName = %q", msg.Name, ddsTypeName)
	g.P("%s_TypeHash = %q", msg.Name, msg.TypeHash)
	g.Out()
	g.P(")")
	g.P("")

	if len(msg.Constants) > 0 {
		g.P("// Message-specific constants")
		g.P("const (")
		g.In()
		for _, c := range msg.Constants {
			g.P("%s_%s = %s", msg.Name, c.Name, c.Value)
		}
		g.Out()
		g.P(")")
		g.P("")
	}
}

func generateTypeNameMethod(g *CodeBuilder, msg MessageDefinition) {
	g.P("// TypeName returns the full ROS 2 type name")
	g.P("func (m *%s) TypeName() string {", msg.Name)
	g.In()
	g.P("return %s_TypeName", msg.Name)
	g.Out()
	g.P("}")
	g.P("")
}

func generateTypeHashMethod(g *CodeBuilder, msg MessageDefinition) {
	g.P("// TypeHash returns the ROS 2 type hash (RIHS01 format)")
	g.P("func (m *%s) TypeHash() string {", msg.Name)
	g.In()
	g.P("return %s_TypeHash", msg.Name)
	g.Out()
	g.P("}")
	g.P("")
}

// fieldNeedsBinary returns true if the field requires the "encoding/binary" import.
func fieldNeedsBinary(field FieldDefinition) bool {
	switch field.FieldType.Kind {
	case "String":
		return true // string length uses binary
	case "Int16", "UInt16", "Int32", "UInt32", "Int64", "UInt64", "Float32", "Float64":
		return true // multi-byte numeric types
	case "Custom", "Time", "Duration":
		if field.IsArray {
			return true // array length prefix uses binary
		}
		// Same-package custom single value: PackToRawAt doesn't use binary in the caller
		return false
	case "Bool", "Int8", "UInt8":
		// Dynamic/bounded arrays have a length prefix that uses binary.
		// Fixed arrays use a simple loop without binary.
		return field.IsArray && field.ArrayKind != "fixed"
	default:
		return false
	}
}

// generatePackField emits CDR-LE pack code for one field, advancing `offset` and
// growing `buf`. Multi-byte primitives and length prefixes are aligned
// according to standard CDR rules.
func generatePackField(g *CodeBuilder, field FieldDefinition) {
	fieldAccess := fmt.Sprintf("m.%s", capitalize(field.Name))

	switch field.FieldType.Kind {
	case "String":
		if field.IsArray {
			g.P("// Pack string array: %s", field.Name)
			g.P("{")
			g.In()
			emitPackAlign(g, 4) // align the array length prefix
			g.P("b := make([]byte, 4)")
			g.P("binary.LittleEndian.PutUint32(b, uint32(len(%s)))", fieldAccess)
			g.P("buf = append(buf, b...)")
			g.P("offset += 4")
			g.P("for _, s := range %s {", fieldAccess)
			g.In()
			emitPackAlign(g, 4) // align each element's length prefix
			g.P("data := []byte(s)")
			g.P("binary.LittleEndian.PutUint32(b, uint32(len(data)+1))")
			g.P("buf = append(buf, b...)")
			g.P("offset += 4")
			g.P("buf = append(buf, data...)")
			g.P("offset += len(data)")
			g.P("buf = append(buf, 0) // null terminator")
			g.P("offset++")
			g.Out()
			g.P("}")
			g.Out()
			g.P("}")
			return
		}
		g.P("// Pack string: %s", field.Name)
		g.P("{")
		g.In()
		emitPackAlign(g, 4)
		g.P("data := []byte(%s)", fieldAccess)
		g.P("b := make([]byte, 4)")
		g.P("binary.LittleEndian.PutUint32(b, uint32(len(data)+1))")
		g.P("buf = append(buf, b...)")
		g.P("offset += 4")
		g.P("buf = append(buf, data...)")
		g.P("offset += len(data)")
		g.P("buf = append(buf, 0) // null terminator")
		g.P("offset++")
		g.Out()
		g.P("}")

	case "Bool", "Int8", "Int16", "Int32", "Int64", "UInt8", "UInt16", "UInt32", "UInt64", "Float32", "Float64":
		g.P("// Pack %s: %s", field.FieldType.Kind, field.Name)
		g.P("{")
		g.In()
		align := cdrAlignment(field.FieldType.Kind)
		// Dynamic sequences require a 4-byte length prefix in CDR.
		if field.IsArray && field.ArrayKind != "fixed" {
			emitPackAlign(g, 4) // length prefix is 4-aligned
			g.P("b := make([]byte, 4)")
			g.P("binary.LittleEndian.PutUint32(b, uint32(len(%s)))", fieldAccess)
			g.P("buf = append(buf, b...)")
			g.P("offset += 4")
		}
		generatePrimitivePackCode(g, field, align)
		g.Out()
		g.P("}")

	case "Custom":
		g.P("// Pack custom type: %s", field.Name)
		g.P("{")
		g.In()
		if field.IsArray {
			emitPackAlign(g, 4)
			g.P("b := make([]byte, 4)")
			g.P("binary.LittleEndian.PutUint32(b, uint32(len(%s)))", fieldAccess)
			g.P("buf = append(buf, b...)")
			g.P("offset += 4")
			g.P("for i := range %s {", fieldAccess)
			g.In()
			g.P("buf, offset = %s[i].PackToRawAt(buf, offset)", fieldAccess)
			g.Out()
			g.P("}")
		} else {
			g.P("buf, offset = %s.PackToRawAt(buf, offset)", fieldAccess)
		}
		g.Out()
		g.P("}")

	case "Time", "Duration":
		g.P("// Pack %s: %s", field.FieldType.Kind, field.Name)
		g.P("{")
		g.In()
		if field.IsArray {
			emitPackAlign(g, 4)
			g.P("b := make([]byte, 4)")
			g.P("binary.LittleEndian.PutUint32(b, uint32(len(%s)))", fieldAccess)
			g.P("buf = append(buf, b...)")
			g.P("offset += 4")
			g.P("for i := range %s {", fieldAccess)
			g.In()
			g.P("buf, offset = %s[i].PackToRawAt(buf, offset)", fieldAccess)
			g.Out()
			g.P("}")
		} else {
			g.P("buf, offset = %s.PackToRawAt(buf, offset)", fieldAccess)
		}
		g.Out()
		g.P("}")
	}
}

// generatePrimitivePackCode emits CDR pack code for a primitive scalar or array.
// align is the per-element alignment; the caller has already emitted any
// length prefix needed for dynamic arrays.
func generatePrimitivePackCode(g *CodeBuilder, field FieldDefinition, align int) {
	fieldAccess := fmt.Sprintf("m.%s", capitalize(field.Name))

	emitOne := func(value string) {
		emitPackAlign(g, align)
		switch field.FieldType.Kind {
		case "Bool":
			g.P("if %s { buf = append(buf, 1) } else { buf = append(buf, 0) }", value)
			g.P("offset++")
		case "Int8":
			g.P("buf = append(buf, byte(%s))", value)
			g.P("offset++")
		case "UInt8":
			g.P("buf = append(buf, byte(%s))", value)
			g.P("offset++")
		case "Int16":
			g.P("{ b := make([]byte, 2); binary.LittleEndian.PutUint16(b, uint16(%s)); buf = append(buf, b...) }", value)
			g.P("offset += 2")
		case "UInt16":
			g.P("{ b := make([]byte, 2); binary.LittleEndian.PutUint16(b, %s); buf = append(buf, b...) }", value)
			g.P("offset += 2")
		case "Int32":
			g.P("{ b := make([]byte, 4); binary.LittleEndian.PutUint32(b, uint32(%s)); buf = append(buf, b...) }", value)
			g.P("offset += 4")
		case "UInt32":
			g.P("{ b := make([]byte, 4); binary.LittleEndian.PutUint32(b, %s); buf = append(buf, b...) }", value)
			g.P("offset += 4")
		case "Int64":
			g.P("{ b := make([]byte, 8); binary.LittleEndian.PutUint64(b, uint64(%s)); buf = append(buf, b...) }", value)
			g.P("offset += 8")
		case "UInt64":
			g.P("{ b := make([]byte, 8); binary.LittleEndian.PutUint64(b, %s); buf = append(buf, b...) }", value)
			g.P("offset += 8")
		case "Float32":
			g.P("{ b := make([]byte, 4); binary.LittleEndian.PutUint32(b, math.Float32bits(%s)); buf = append(buf, b...) }", value)
			g.P("offset += 4")
		case "Float64":
			g.P("{ b := make([]byte, 8); binary.LittleEndian.PutUint64(b, math.Float64bits(%s)); buf = append(buf, b...) }", value)
			g.P("offset += 8")
		}
	}

	if !field.IsArray {
		emitOne(fieldAccess)
		return
	}

	// Array path. Fast paths for byte-wide types when the slice memory layout
	// matches []byte (no per-element alignment needed since align==1).
	if field.FieldType.Kind == "UInt8" {
		if field.ArrayKind == "fixed" {
			// [N]uint8 (fixed array) needs [:] to convert to slice.
			g.P("buf = append(buf, %s[:]...)", fieldAccess)
			g.P("offset += len(%s)", fieldAccess)
		} else {
			// []uint8 == []byte, safe direct append.
			g.P("buf = append(buf, %s...)", fieldAccess)
			g.P("offset += len(%s)", fieldAccess)
		}
		return
	}
	g.P("for _, v := range %s {", fieldAccess)
	g.In()
	emitOne("v")
	g.Out()
	g.P("}")
}

// generateUnpackField emits CDR-LE unpack code for one field, advancing the
// shared `offset` and reading from `data`. Multi-byte primitives and length
// prefixes are aligned according to standard CDR rules. Assumes a `var err
// error` is in scope when the field is a Custom/Time/Duration kind.
func generateUnpackField(g *CodeBuilder, field FieldDefinition) {
	fieldAccess := fmt.Sprintf("m.%s", capitalize(field.Name))

	switch field.FieldType.Kind {
	case "String":
		if field.IsArray {
			g.P("// Unpack string array: %s", field.Name)
			g.P("{")
			g.In()
			emitUnpackAlign(g, 4)
			g.P("if offset+4 > len(data) { return offset, fmt.Errorf(\"buffer too short for string array length\") }")
			g.P("arrLen := int(binary.LittleEndian.Uint32(data[offset:]))")
			g.P("offset += 4")
			g.P("%s = make([]string, arrLen)", fieldAccess)
			g.P("for i := 0; i < arrLen; i++ {")
			g.In()
			emitUnpackAlign(g, 4)
			g.P("if offset+4 > len(data) { return offset, fmt.Errorf(\"buffer too short for string length\") }")
			g.P("strLen := int(binary.LittleEndian.Uint32(data[offset:]))")
			g.P("offset += 4")
			g.P("if strLen > 0 {")
			g.In()
			g.P("if offset+strLen > len(data) { return offset, fmt.Errorf(\"buffer too short for string data\") }")
			g.P("%s[i] = string(data[offset:offset+strLen-1]) // exclude null terminator", fieldAccess)
			g.P("offset += strLen")
			g.Out()
			g.P("}")
			g.Out()
			g.P("}")
			g.Out()
			g.P("}")
			return
		}
		g.P("// Unpack string: %s", field.Name)
		g.P("{")
		g.In()
		emitUnpackAlign(g, 4)
		g.P("if offset+4 > len(data) { return offset, fmt.Errorf(\"buffer too short for string length\") }")
		g.P("strLen := int(binary.LittleEndian.Uint32(data[offset:]))")
		g.P("offset += 4")
		g.P("if strLen > 0 {")
		g.In()
		g.P("if offset+strLen > len(data) { return offset, fmt.Errorf(\"buffer too short for string data\") }")
		g.P("%s = string(data[offset:offset+strLen-1]) // exclude null terminator", fieldAccess)
		g.P("offset += strLen")
		g.Out()
		g.P("}")
		g.Out()
		g.P("}")

	case "Bool", "Int8", "Int16", "Int32", "Int64", "UInt8", "UInt16", "UInt32", "UInt64", "Float32", "Float64":
		g.P("// Unpack %s: %s", field.FieldType.Kind, field.Name)
		g.P("{")
		g.In()
		generatePrimitiveUnpackCode(g, field, cdrAlignment(field.FieldType.Kind))
		g.Out()
		g.P("}")

	case "Custom":
		g.P("// Unpack custom type: %s", field.Name)
		g.P("{")
		g.In()
		if field.IsArray {
			emitUnpackAlign(g, 4)
			g.P("if offset+4 > len(data) { return offset, fmt.Errorf(\"buffer too short for array length\") }")
			g.P("arrLen := int(binary.LittleEndian.Uint32(data[offset:]))")
			g.P("offset += 4")
			g.P("%s = make([]%s, arrLen)", fieldAccess, fieldToGoBaseTypeInPkg(field, g.currentPkg))
			g.P("for i := 0; i < arrLen; i++ {")
			g.In()
			g.P("offset, err = %s[i].UnpackFromRawAt(data, offset)", fieldAccess)
			g.P("if err != nil { return offset, err }")
			g.Out()
			g.P("}")
		} else {
			g.P("offset, err = %s.UnpackFromRawAt(data, offset)", fieldAccess)
			g.P("if err != nil { return offset, err }")
		}
		g.Out()
		g.P("}")

	case "Time", "Duration":
		g.P("// Unpack %s: %s", field.FieldType.Kind, field.Name)
		g.P("{")
		g.In()
		if field.IsArray {
			emitUnpackAlign(g, 4)
			g.P("if offset+4 > len(data) { return offset, fmt.Errorf(\"buffer too short for array length\") }")
			g.P("arrLen := int(binary.LittleEndian.Uint32(data[offset:]))")
			g.P("offset += 4")
			g.P("%s = make([]%s, arrLen)", fieldAccess, fieldToGoBaseTypeInPkg(field, g.currentPkg))
			g.P("for i := 0; i < arrLen; i++ {")
			g.In()
			g.P("offset, err = %s[i].UnpackFromRawAt(data, offset)", fieldAccess)
			g.P("if err != nil { return offset, err }")
			g.Out()
			g.P("}")
		} else {
			g.P("offset, err = %s.UnpackFromRawAt(data, offset)", fieldAccess)
			g.P("if err != nil { return offset, err }")
		}
		g.Out()
		g.P("}")
	}
}

// generatePrimitiveUnpackCode emits CDR unpack code for a primitive scalar or
// array field. align is the per-element alignment.
func generatePrimitiveUnpackCode(g *CodeBuilder, field FieldDefinition, align int) {
	fieldAccess := fmt.Sprintf("m.%s", capitalize(field.Name))

	emitOne := func(target string) {
		emitUnpackAlign(g, align)
		switch field.FieldType.Kind {
		case "Bool":
			g.P("if offset >= len(data) { return offset, fmt.Errorf(\"buffer too short for bool\") }")
			g.P("%s = data[offset] != 0", target)
			g.P("offset++")
		case "Int8":
			g.P("if offset >= len(data) { return offset, fmt.Errorf(\"buffer too short for int8\") }")
			g.P("%s = int8(data[offset])", target)
			g.P("offset++")
		case "UInt8":
			g.P("if offset >= len(data) { return offset, fmt.Errorf(\"buffer too short for uint8\") }")
			g.P("%s = data[offset]", target)
			g.P("offset++")
		case "Int16":
			g.P("if offset+2 > len(data) { return offset, fmt.Errorf(\"buffer too short for int16\") }")
			g.P("%s = int16(binary.LittleEndian.Uint16(data[offset:]))", target)
			g.P("offset += 2")
		case "UInt16":
			g.P("if offset+2 > len(data) { return offset, fmt.Errorf(\"buffer too short for uint16\") }")
			g.P("%s = binary.LittleEndian.Uint16(data[offset:])", target)
			g.P("offset += 2")
		case "Int32":
			g.P("if offset+4 > len(data) { return offset, fmt.Errorf(\"buffer too short for int32\") }")
			g.P("%s = int32(binary.LittleEndian.Uint32(data[offset:]))", target)
			g.P("offset += 4")
		case "UInt32":
			g.P("if offset+4 > len(data) { return offset, fmt.Errorf(\"buffer too short for uint32\") }")
			g.P("%s = binary.LittleEndian.Uint32(data[offset:])", target)
			g.P("offset += 4")
		case "Int64":
			g.P("if offset+8 > len(data) { return offset, fmt.Errorf(\"buffer too short for int64\") }")
			g.P("%s = int64(binary.LittleEndian.Uint64(data[offset:]))", target)
			g.P("offset += 8")
		case "UInt64":
			g.P("if offset+8 > len(data) { return offset, fmt.Errorf(\"buffer too short for uint64\") }")
			g.P("%s = binary.LittleEndian.Uint64(data[offset:])", target)
			g.P("offset += 8")
		case "Float32":
			g.P("if offset+4 > len(data) { return offset, fmt.Errorf(\"buffer too short for float32\") }")
			g.P("%s = math.Float32frombits(binary.LittleEndian.Uint32(data[offset:]))", target)
			g.P("offset += 4")
		case "Float64":
			g.P("if offset+8 > len(data) { return offset, fmt.Errorf(\"buffer too short for float64\") }")
			g.P("%s = math.Float64frombits(binary.LittleEndian.Uint64(data[offset:]))", target)
			g.P("offset += 8")
		}
	}

	if !field.IsArray {
		emitOne(fieldAccess)
		return
	}

	// Array path
	switch field.ArrayKind {
	case "fixed":
		g.P("for i := 0; i < %d; i++ {", *field.ArraySize)
		g.In()
		emitOne(fmt.Sprintf("%s[i]", fieldAccess))
		g.Out()
		g.P("}")
	default: // unbounded or bounded: read the length prefix here
		emitUnpackAlign(g, 4)
		g.P("if offset+4 > len(data) { return offset, fmt.Errorf(\"buffer too short for array length\") }")
		g.P("arrLen := int(binary.LittleEndian.Uint32(data[offset:]))")
		g.P("offset += 4")
		elemType := fieldToGoBaseTypeInPkg(field, g.currentPkg)
		g.P("%s = make([]%s, arrLen)", fieldAccess, elemType)
		g.P("for i := 0; i < arrLen; i++ {")
		g.In()
		emitOne(fmt.Sprintf("%s[i]", fieldAccess))
		g.Out()
		g.P("}")
	}
}

func fieldToGoBaseType(field FieldDefinition) string {
	return fieldToGoBaseTypeInPkg(field, "")
}

func fieldToGoBaseTypeInPkg(field FieldDefinition, currentPkg string) string {
	switch field.FieldType.Kind {
	case "Bool":
		return "bool"
	case "Int8":
		return "int8"
	case "Int16":
		return "int16"
	case "Int32":
		return "int32"
	case "Int64":
		return "int64"
	case "UInt8":
		return "uint8"
	case "UInt16":
		return "uint16"
	case "UInt32":
		return "uint32"
	case "UInt64":
		return "uint64"
	case "Float32":
		return "float32"
	case "Float64":
		return "float64"
	case "String":
		return "string"
	case "Custom":
		pkg := sanitizePackageName(*field.FieldType.Package)
		if pkg == currentPkg {
			return *field.FieldType.Name
		}
		return fmt.Sprintf("%s.%s", pkg, *field.FieldType.Name)
	default:
		return "interface{}"
	}
}

func GenerateGoService(srv ServiceDefinition, prefix string) ([]byte, error) {
	pkgName := sanitizePackageName(srv.Package)
	g := &CodeBuilder{currentPkg: pkgName}
	g.P("package %s", pkgName)
	g.P("")

	// Analyze imports needed
	needsMath := false
	needsBinary := false
	crossPkgImports := map[string]bool{}
	allFields := append(srv.Request.Fields, srv.Response.Fields...)
	for _, field := range allFields {
		if field.FieldType.Kind == "Float32" || field.FieldType.Kind == "Float64" {
			needsMath = true
		}
		if fieldNeedsBinary(field) {
			needsBinary = true
		}
		if field.FieldType.Kind == "Custom" && field.FieldType.Package != nil {
			pkg := sanitizePackageName(*field.FieldType.Package)
			if pkg != pkgName {
				crossPkgImports[pkg] = true
			}
		}
		if field.FieldType.Kind == "Time" || field.FieldType.Kind == "Duration" {
			if pkgName != "builtin_interfaces" {
				crossPkgImports["builtin_interfaces"] = true
				needsBinary = true
			}
		}
	}

	// Imports
	g.P("import (")
	g.In()
	if needsBinary {
		g.P(`"encoding/binary"`)
	}
	g.P(`"fmt"`)
	if needsMath {
		g.P(`"math"`)
	}
	g.P(`"%s/rosz"`, prefix)
	for pkg := range crossPkgImports {
		g.P(`"%s/generated/%s"`, prefix, pkg)
	}
	g.Out()
	g.P(")")
	g.P("")

	g.P("// %s is a ROS 2 service type", srv.Name)
	g.P("// Full name: %s", srv.FullName)
	g.P("")

	// Use DDS-qualified service type name so Zenoh key expressions match rmw_zenoh_cpp and ros-z Rust.
	ddsSvcTypeName := fmt.Sprintf("%s::srv::dds_::%s_", srv.Package, srv.Name)
	g.P("const (")
	g.In()
	g.P("%s_TypeName = %q", srv.Name, ddsSvcTypeName)
	g.P("%s_TypeHash = %q", srv.Name, srv.TypeHash)
	g.Out()
	g.P(")")
	g.P("")

	reqName := srv.Name + "Request"
	respName := srv.Name + "Response"

	// Service marker type implementing the rosz.Service interface
	g.P("// %s represents the service type marker", srv.Name)
	g.P("type %s struct{}", srv.Name)
	g.P("")

	g.P("func (s *%s) TypeName() string { return %s_TypeName }", srv.Name, srv.Name)
	g.P("func (s *%s) TypeHash() string { return %s_TypeHash }", srv.Name, srv.Name)
	g.P("func (s *%s) SerializeCDR() ([]byte, error) { return nil, nil }", srv.Name)
	g.P("func (s *%s) DeserializeCDR(_ []byte) error { return nil }", srv.Name)
	g.P("func (s *%s) GetRequest() rosz.Message { return &%s{} }", srv.Name, reqName)
	g.P("func (s *%s) GetResponse() rosz.Message { return &%s{} }", srv.Name, respName)
	g.P("")

	// Generate Request type
	generateServiceMessage(g, reqName, srv.Request, pkgName)

	// Generate Response type
	generateServiceMessage(g, respName, srv.Response, pkgName)

	return g.Bytes()
}

func generateServiceMessage(g *CodeBuilder, name string, msg MessageDefinition, currentPkg string) {
	// Generate struct
	g.P("// %s is the request/response message for the service", name)
	g.P("type %s struct {", name)
	g.In()
	for _, field := range msg.Fields {
		goType := fieldToGoTypeInPkg(field, currentPkg)
		g.P("%s %s", capitalize(field.Name), goType)
	}
	g.Out()
	g.P("}")
	g.P("")

	// Generate constants
	g.P("const (")
	g.In()
	g.P("%s_TypeName = %q", name, msg.FullName)
	g.P("%s_TypeHash = %q", name, msg.TypeHash)
	g.Out()
	g.P(")")
	g.P("")

	// Generate TypeName method
	g.P("func (m *%s) TypeName() string { return %s_TypeName }", name, name)
	g.P("func (m *%s) TypeHash() string { return %s_TypeHash }", name, name)
	g.P("")

	emitMessageCodec(g, name, msg.Fields)
}

func fieldToGoType(field FieldDefinition) string {
	return fieldToGoTypeInPkg(field, "")
}

func fieldToGoTypeInPkg(field FieldDefinition, currentPkg string) string {
	var baseType string

	switch field.FieldType.Kind {
	case "Bool":
		baseType = "bool"
	case "Int8":
		baseType = "int8"
	case "Int16":
		baseType = "int16"
	case "Int32":
		baseType = "int32"
	case "Int64":
		baseType = "int64"
	case "UInt8":
		baseType = "uint8"
	case "UInt16":
		baseType = "uint16"
	case "UInt32":
		baseType = "uint32"
	case "UInt64":
		baseType = "uint64"
	case "Float32":
		baseType = "float32"
	case "Float64":
		baseType = "float64"
	case "String":
		baseType = "string"
	case "Time":
		if currentPkg == "builtin_interfaces" {
			baseType = "Time"
		} else {
			baseType = "builtin_interfaces.Time"
		}
	case "Duration":
		if currentPkg == "builtin_interfaces" {
			baseType = "Duration"
		} else {
			baseType = "builtin_interfaces.Duration"
		}
	case "Custom":
		pkg := sanitizePackageName(*field.FieldType.Package)
		if pkg == currentPkg {
			baseType = *field.FieldType.Name
		} else {
			baseType = fmt.Sprintf("%s.%s", pkg, *field.FieldType.Name)
		}
	default:
		baseType = "interface{}"
	}

	if field.IsArray {
		switch field.ArrayKind {
		case "fixed":
			return fmt.Sprintf("[%d]%s", *field.ArraySize, baseType)
		default:
			return "[]" + baseType
		}
	}

	return baseType
}

// GenerateGoAction generates Go code for a ROS 2 action type.
// It produces the action marker type, Goal/Result/Feedback sub-types with CDR serialization,
// and implements ActionSubServiceHashes for interop with rmw_zenoh_cpp.
func GenerateGoAction(action ActionDefinition, prefix string) ([]byte, error) {
	pkgName := sanitizePackageName(action.Package)
	g := &CodeBuilder{currentPkg: pkgName}
	g.P("package %s", pkgName)
	g.P("")

	// Collect imports needed by sub-messages
	needsMath := false
	needsBinary := false
	crossPkgImports := map[string]bool{}
	var allSubFields []FieldDefinition
	allSubFields = append(allSubFields, action.Goal.Fields...)
	if action.Result != nil {
		allSubFields = append(allSubFields, action.Result.Fields...)
	}
	if action.Feedback != nil {
		allSubFields = append(allSubFields, action.Feedback.Fields...)
	}
	for _, field := range allSubFields {
		if field.FieldType.Kind == "Float32" || field.FieldType.Kind == "Float64" {
			needsMath = true
		}
		if fieldNeedsBinary(field) {
			needsBinary = true
		}
		if field.FieldType.Kind == "Custom" && field.FieldType.Package != nil {
			pkg := sanitizePackageName(*field.FieldType.Package)
			if pkg != pkgName {
				crossPkgImports[pkg] = true
			}
		}
		if field.FieldType.Kind == "Time" || field.FieldType.Kind == "Duration" {
			if pkgName != "builtin_interfaces" {
				crossPkgImports["builtin_interfaces"] = true
				needsBinary = true
			}
		}
	}

	g.P("import (")
	g.In()
	if needsBinary {
		g.P(`"encoding/binary"`)
	}
	g.P(`"fmt"`)
	if needsMath {
		g.P(`"math"`)
	}
	g.P(`"%s/rosz"`, prefix)
	for pkg := range crossPkgImports {
		g.P(`"%s/generated/%s"`, prefix, pkg)
	}
	g.Out()
	g.P(")")
	g.P("")

	g.P("// %s is a ROS 2 action type", action.Name)
	g.P("// Full name: %s", action.FullName)
	g.P("")

	// Action-level constants: TypeName uses the ros-z internal format (not DDS).
	// Sub-service compound hashes are the DDS RIHS01 hashes used by rmw_zenoh_cpp.
	g.P("const (")
	g.In()
	g.P("%s_TypeName = %q", action.Name, action.FullName)
	g.P("%s_SendGoalHash = %q", action.Name, action.SendGoalHash)
	g.P("%s_GetResultHash = %q", action.Name, action.GetResultHash)
	g.P("%s_CancelGoalHash = %q", action.Name, action.CancelGoalHash)
	g.P("%s_FeedbackMessageHash = %q", action.Name, action.FeedbackMessageHash)
	g.P("%s_StatusHash = %q", action.Name, action.StatusHash)
	g.Out()
	g.P(")")
	g.P("")

	// Action marker type
	g.P("// %s is the action type marker", action.Name)
	g.P("type %s struct{}", action.Name)
	g.P("")

	// Action interface methods
	goalName := action.Name + "Goal"
	resultName := action.Name + "Result"
	feedbackName := action.Name + "Feedback"

	g.P("func (a *%s) TypeName() string      { return %s_TypeName }", action.Name, action.Name)
	g.P("func (a *%s) GetGoal() rosz.Message { return &%s{} }", action.Name, goalName)
	if action.Result != nil {
		g.P("func (a *%s) GetResult() rosz.Message     { return &%s{} }", action.Name, resultName)
	} else {
		g.P("func (a *%s) GetResult() rosz.Message     { return nil }", action.Name)
	}
	if action.Feedback != nil {
		g.P("func (a *%s) GetFeedback() rosz.Message   { return &%s{} }", action.Name, feedbackName)
	} else {
		g.P("func (a *%s) GetFeedback() rosz.Message   { return nil }", action.Name)
	}
	g.P("")

	// Action interface sub-service hash methods — provide compound hashes for rmw_zenoh_cpp interop
	g.P("// SendGoalHash returns the compound RIHS01 hash for the SendGoal sub-service.")
	g.P("func (a *%s) SendGoalHash() string        { return %s_SendGoalHash }", action.Name, action.Name)
	g.P("// GetResultHash returns the compound RIHS01 hash for the GetResult sub-service.")
	g.P("func (a *%s) GetResultHash() string       { return %s_GetResultHash }", action.Name, action.Name)
	g.P("// CancelGoalHash returns the compound RIHS01 hash for the CancelGoal sub-service.")
	g.P("func (a *%s) CancelGoalHash() string      { return %s_CancelGoalHash }", action.Name, action.Name)
	g.P("// FeedbackMessageHash returns the compound RIHS01 hash for the FeedbackMessage topic.")
	g.P("func (a *%s) FeedbackMessageHash() string { return %s_FeedbackMessageHash }", action.Name, action.Name)
	g.P("// StatusHash returns the compound RIHS01 hash for the GoalStatusArray topic.")
	g.P("func (a *%s) StatusHash() string          { return %s_StatusHash }", action.Name, action.Name)
	g.P("")

	// Generate Goal sub-type
	generateActionSubMessage(g, goalName, action.Goal, pkgName)

	// Generate Result sub-type (if present)
	if action.Result != nil {
		generateActionSubMessage(g, resultName, *action.Result, pkgName)
	}

	// Generate Feedback sub-type (if present)
	if action.Feedback != nil {
		generateActionSubMessage(g, feedbackName, *action.Feedback, pkgName)
	}

	return g.Bytes()
}

// generateActionSubMessage generates a Goal/Result/Feedback sub-message type for an action.
func generateActionSubMessage(g *CodeBuilder, name string, msg MessageDefinition, currentPkg string) {
	comment := "goal"
	switch {
	case len(name) > 6 && name[len(name)-6:] == "Result":
		comment = "result"
	case len(name) > 8 && name[len(name)-8:] == "Feedback":
		comment = "feedback"
	}
	g.P("// %s is the %s message for the action", name, comment)
	g.P("type %s struct {", name)
	g.In()
	for _, field := range msg.Fields {
		goType := fieldToGoTypeInPkg(field, currentPkg)
		g.P("%s %s", capitalize(field.Name), goType)
	}
	g.Out()
	g.P("}")
	g.P("")

	// Constants (use the message-level full_name and type_hash)
	g.P("const (")
	g.In()
	g.P("%s_TypeName = %q", name, msg.FullName)
	g.P("%s_TypeHash = %q", name, msg.TypeHash)
	g.Out()
	g.P(")")
	g.P("")

	g.P("func (m *%s) TypeName() string { return %s_TypeName }", name, name)
	g.P("func (m *%s) TypeHash() string { return %s_TypeHash }", name, name)
	g.P("")

	emitMessageCodec(g, name, msg.Fields)
}

func capitalize(s string) string {
	if len(s) == 0 {
		return s
	}
	runes := []rune(s)
	runes[0] = unicode.ToUpper(runes[0])
	return string(runes)
}
