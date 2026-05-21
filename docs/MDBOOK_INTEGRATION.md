# mdBook Integration Instructions

This file explains how to integrate the Go bindings documentation into the main repository's mdbook.

## Files Created

1. **`_tmp/go_bindings_mdbook.md`** - Complete updated chapter for Go bindings
   - Replaces `book/src/chapters/go_bindings.md` in main repo
   - 450+ lines of comprehensive documentation
   - Includes all Phase 1 & 2 improvements

2. **`_tmp/go_bindings_updates.md`** - Detailed section updates (reference)
   - Individual sections that were added
   - Can be used to selectively update specific parts

3. **`_tmp/go_bindings_original.md`** - Original chapter from commit 2aaefd8
   - Backup for reference

## Integration Steps

### Option 1: Full Replacement (Recommended)

```bash
# In main repository (not worktree)
cd /path/to/ros-z  # Main repo
cp /path/to/worktree/ros-z-go/_tmp/go_bindings_mdbook.md book/src/chapters/go_bindings.md

# Build and preview
mdbook serve book

# Commit
git add book/src/chapters/go_bindings.md
git commit -m "docs(go): update Go bindings chapter with Phase 1 & 2 improvements"
```

### Option 2: Merge from Worktree Branch

```bash
# In main repository
git fetch origin dev/ros-z-go

# Cherry-pick the documentation commit (if applicable)
# OR manually copy the file and commit
```

## What's New in the Updated Chapter

### Added Sections

1. **Memory Safety** (new)
   - cgo.Handle explanation
   - runtime.Pinner usage
   - Automatic memory management

2. **Error Handling** (new major section)
   - RoszError type
   - Error codes reference
   - Convenience methods (IsTimeout, IsRejected)
   - Retry patterns

3. **Handler Interface** (new major section)
   - Three delivery patterns (Closure, FifoChannel, RingChannel)
   - Comparison table
   - When to use each pattern

4. **Enhanced Examples** (expanded)
   - Added channel-based subscriber example
   - Added error handling examples
   - Added concurrent processing patterns

5. **Performance Considerations** (new)
   - Callback vs channel tradeoffs
   - Latency guidance
   - Recommendations

6. **Migration Guide** (new)
   - v0.1 → v0.2+ migration
   - Opt-in new features
   - No breaking changes

7. **Troubleshooting** (enhanced)
   - CGO linker errors
   - Type hash mismatches
   - Performance issues

### Updated Sections

- **Installation**: Updated prerequisites (Go 1.23+)
- **Quick Start**: Added error handling to examples
- **Services**: Added error handling examples
- **Actions**: Added goal rejection handling
- **Testing**: Added test organization info

## mdBook Table of Contents

No changes needed to `book/src/SUMMARY.md` - the Go Bindings chapter already exists:

```markdown
# Experimental

- [Go Bindings](./chapters/go_bindings.md)
```

## Verification

After integrating, verify:

1. **Build mdbook**:

   ```bash
   mdbook build book
   # Check for errors
   ```

2. **Serve locally**:

   ```bash
   mdbook serve book
   # Open http://localhost:3000
   # Navigate to Experimental → Go Bindings
   ```

3. **Check formatting**:
   - Mermaid diagrams render correctly
   - Admonish boxes display properly
   - Code blocks have syntax highlighting
   - Links work (especially relative paths)

4. **Test code examples**:
   - Copy/paste examples should compile
   - Error handling patterns should be correct
   - Import paths should be accurate

## Related Commits

This documentation reflects the following commits from dev/ros-z-go:

- `ee959c9` - refactor(go): use cgo.Handle and runtime.Pinner for callbacks
- `1032a34` - feat(go): add structured error type with FFI error codes
- `1e4b4b5` - feat(go): add Handler interface with channel delivery options
- `c0b1bf2` - docs(go): add comprehensive README for ros-z-go package
- `0e3eea6` - docs(go): add enhanced examples showcasing new features

## Notes

- The updated chapter is **450+ lines** (vs 310 lines original)
- All new features are documented with examples
- Maintains existing structure and flow
- Backward compatible (no breaking changes)
- Production-ready content (tested examples)
