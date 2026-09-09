# Installation & Development

## Quick Local Install (Testing)

### Option 1: Symlink to extensions folder

```bash
# macOS
ln -s $(pwd) ~/.vscode/extensions/dyard-syntax-0.1.0

# Linux
ln -s $(pwd) ~/.vscode/extensions/dyard-syntax-0.1.0

# Windows (PowerShell as Admin)
New-Item -ItemType SymbolicLink -Path $env:APPDATA\.vscode\extensions\dyard-syntax-0.1.0 -Target (Get-Location)
```

Then restart VS Code.

### Option 2: Copy directly

```bash
# macOS/Linux
cp -r . ~/.vscode/extensions/dyard-syntax-0.1.0

# Windows
xcopy . %APPDATA%\.vscode\extensions\dyard-syntax-0.1.0 /E /I
```

Then restart VS Code.

### Option 3: Use VS Code Extensions folder

1. Open VS Code
2. Press `Cmd+Shift+P` (macOS) or `Ctrl+Shift+P` (Linux/Windows)
3. Type "Developer: Open Extensions Folder"
4. Copy this folder into the extensions directory
5. Restart VS Code

## Verify Installation

1. Create a test file: `test.dyard`
2. Add content:
   ```dyard
   container image nginx:latest as web
   limit to 512m, 1cpu
   ```
3. Check if syntax highlighting is applied (different colors)

## Development

### Edit syntax rules

- `.dyard` syntax: `syntaxes/dyard.tmLanguage.json`
- `config.dyard` syntax: `syntaxes/config-dyard.tmLanguage.json`

### Edit snippets

- `snippets/dyard.json` - Add more snippets

### Edit language config

- `language-configuration/dyard.json` - Bracket pairs, indentation, etc.

### Test changes

1. Restart VS Code after changes
2. Open a `.dyard` file
3. Verify highlighting

## Publishing to VS Code Marketplace

### Prerequisites

```bash
npm install -g vsce
```

### Build package

```bash
cd vscode-extension
vsce package
```

This creates `dyard-syntax-0.1.0.vsix`

### Publish

```bash
vsce publish
```

Or manually upload to https://marketplace.visualstudio.com

## Troubleshooting

### Syntax highlighting not working

1. Check file extension (`.dyard` or `config.dyard`)
2. Reload VS Code window (`Cmd+R` on macOS)
3. Check syntax file paths in `package.json`

### Grammar not parsing correctly

1. Validate JSON in syntax files
2. Check scope names match in grammars section
3. Review TextMate grammar patterns

### View tokenized output (debug)

```
Cmd+Shift+P → "Developer: Inspect Tokens"
```

Then place cursor on tokens to see their scope.
