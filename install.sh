#!/usr/bin/env bash
# DevDock Installation Script
# Usage: curl -fsSL https://raw.githubusercontent.com/aryan-agrawal-glean/devdock/main/install.sh | bash
#    or: git clone https://github.com/aryan-agrawal-glean/devdock && cd devdock && ./install.sh

set -e

# Configuration
REPO="aryan-agrawal-glean/devdock"
INSTALL_DIR="${DEVDOCK_INSTALL_DIR:-/usr/local/bin}"
BINARY_NAME="devdock"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

info() {
    echo -e "${BLUE}ℹ${NC} $1" >&2
}

success() {
    echo -e "${GREEN}✓${NC} $1" >&2
}

warning() {
    echo -e "${YELLOW}⚠${NC} $1" >&2
}

error() {
    echo -e "${RED}✗${NC} $1" >&2
    exit 1
}

# Detect best available download tool: curl (preferred) or gh CLI (fallback)
detect_download_tool() {
    if command -v curl >/dev/null 2>&1; then
        echo "curl"
    elif command -v gh >/dev/null 2>&1; then
        echo "gh"
    else
        error "Neither curl nor GitHub CLI (gh) is installed.\nInstall curl or gh: https://cli.github.com/"
    fi
}

# Detect OS and architecture
detect_platform() {
    local os=""
    local arch=""

    case "$(uname -s)" in
        Linux*)
            os="linux"
            ;;
        Darwin*)
            os="macos"
            ;;
        *)
            error "Unsupported operating system: $(uname -s)"
            ;;
    esac

    case "$(uname -m)" in
        x86_64|amd64)
            arch="x86_64"
            ;;
        arm64|aarch64)
            if [ "$os" = "macos" ]; then
                arch="arm64"
            else
                error "Linux ARM64 is not currently supported"
            fi
            ;;
        *)
            error "Unsupported architecture: $(uname -m)"
            ;;
    esac

    echo "${os}-${arch}"
}

# Get latest release version (curl preferred, gh fallback)
get_latest_version() {
    local download_tool="$1"
    local version

    if [ "$download_tool" = "curl" ]; then
        # Use GitHub API (no auth required for public repos)
        version=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' \
            | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')
    else
        version=$(gh release list --repo "$REPO" --limit 1 --json tagName --jq '.[0].tagName')
    fi

    if [ -z "$version" ]; then
        error "Failed to fetch latest version from GitHub"
    fi

    echo "$version"
}

# Download binary (curl preferred, gh fallback)
download_binary() {
    local version="$1"
    local platform="$2"
    local download_tool="$3"
    local asset_name="${BINARY_NAME}-${platform}-${version}.tar.gz"
    local temp_dir
    temp_dir=$(mktemp -d)

    info "Downloading DevDock ${version} for ${platform}..."

    if [ "$download_tool" = "curl" ]; then
        local url="https://github.com/${REPO}/releases/download/${version}/${asset_name}"
        if ! curl -fsSL "$url" -o "${temp_dir}/${asset_name}"; then
            error "Failed to download binary from ${url}"
        fi
    else
        if ! gh release download "$version" --repo "$REPO" --pattern "$asset_name" --dir "$temp_dir"; then
            error "Failed to download binary. Available assets:"
            gh release view "$version" --repo "$REPO" --json assets --jq '.assets[].name'
            exit 1
        fi
    fi

    # Verify the download succeeded
    if [ ! -f "${temp_dir}/${asset_name}" ]; then
        error "Downloaded file not found: ${asset_name}"
    fi

    # Extract the tarball
    info "Extracting archive..."
    if ! tar -xzf "${temp_dir}/${asset_name}" -C "$temp_dir"; then
        error "Failed to extract archive"
    fi

    # Remove the tarball
    rm "${temp_dir}/${asset_name}"

    # Verify it's a valid executable (basic check)
    if ! file "${temp_dir}/${BINARY_NAME}" | grep -q "executable"; then
        warning "Downloaded file may not be a valid executable:"
        file "${temp_dir}/${BINARY_NAME}" >&2
    fi

    echo "$temp_dir"
}

# Install binary with proper permissions and macOS support
install_binary() {
    local temp_dir="$1"
    local needs_sudo=false

    if [ ! -w "$INSTALL_DIR" ]; then
        needs_sudo=true
        warning "Installation directory ${INSTALL_DIR} requires sudo access"
    fi

    chmod +x "${temp_dir}/${BINARY_NAME}"

    info "Installing to ${INSTALL_DIR}/${BINARY_NAME}..."

    # Use install command for atomic operation with correct permissions
    if [ "$needs_sudo" = true ]; then
        if ! command -v sudo >/dev/null 2>&1; then
            error "sudo is required but not found."
        fi
        sudo install -m 755 "${temp_dir}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
    else
        install -m 755 "${temp_dir}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
    fi

    # On macOS, remove quarantine attribute to avoid Gatekeeper issues
    if [ "$(uname -s)" = "Darwin" ]; then
        info "Removing macOS quarantine attribute..."
        if [ "$needs_sudo" = true ]; then
            sudo xattr -d com.apple.quarantine "${INSTALL_DIR}/${BINARY_NAME}" 2>/dev/null || true
        else
            xattr -d com.apple.quarantine "${INSTALL_DIR}/${BINARY_NAME}" 2>/dev/null || true
        fi
    fi

    rm -rf "$temp_dir"

    # Verify the binary actually works before declaring success
    if ! "${INSTALL_DIR}/${BINARY_NAME}" --version >/dev/null 2>&1; then
        error "Installed binary is not executable or corrupt. Try downloading again."
    fi

    success "DevDock installed successfully!"
}

# Check if binary is in PATH
check_path() {
    if ! echo "$PATH" | grep -q "$INSTALL_DIR"; then
        warning "${INSTALL_DIR} is not in your PATH"
        echo "" >&2

        # Detect shell and config file
        local shell_config=""
        if [ -n "$BASH_VERSION" ]; then
            if [ -f "$HOME/.bash_profile" ]; then
                shell_config="$HOME/.bash_profile"
            elif [ -f "$HOME/.bashrc" ]; then
                shell_config="$HOME/.bashrc"
            fi
        elif [ -n "$ZSH_VERSION" ]; then
            shell_config="$HOME/.zshrc"
        else
            # Try to detect from SHELL env var
            case "$SHELL" in
                */zsh)
                    shell_config="$HOME/.zshrc"
                    ;;
                */bash)
                    if [ -f "$HOME/.bash_profile" ]; then
                        shell_config="$HOME/.bash_profile"
                    else
                        shell_config="$HOME/.bashrc"
                    fi
                    ;;
            esac
        fi

        if [ -n "$shell_config" ]; then
            echo "Would you like to add ${INSTALL_DIR} to your PATH automatically?" >&2
            read -p "Update $shell_config? [y/N] " -n 1 -r
            echo "" >&2

            if [[ $REPLY =~ ^[Yy]$ ]]; then
                # Check if already in config (avoid duplicates)
                if grep -q "export PATH=\"${INSTALL_DIR}:\$PATH\"" "$shell_config" 2>/dev/null; then
                    info "PATH export already exists in $shell_config"
                else
                    echo "" >> "$shell_config"
                    echo "# Added by DevDock installer" >> "$shell_config"
                    echo "export PATH=\"${INSTALL_DIR}:\$PATH\"" >> "$shell_config"
                    success "Updated $shell_config"
                    echo "" >&2
                    warning "Please restart your shell or run:"
                    echo "  source $shell_config" >&2
                fi
            else
                echo "Add it manually to your shell profile:" >&2
                echo "  export PATH=\"${INSTALL_DIR}:\$PATH\"" >&2
            fi
        else
            echo "Add it to your shell profile:" >&2
            echo "  export PATH=\"${INSTALL_DIR}:\$PATH\"" >&2
        fi
        echo "" >&2
    fi
}

# Main installation flow
main() {
    echo ""
    echo "╔════════════════════════════════════════╗"
    echo "║       DevDock Installation Script      ║"
    echo "╚════════════════════════════════════════╝"
    echo ""

    # Detect best download tool (curl preferred, gh fallback)
    local download_tool
    download_tool=$(detect_download_tool)
    info "Download method: ${download_tool}"

    # Detect platform
    local platform
    platform=$(detect_platform)
    info "Detected platform: ${platform}"

    # Get latest version
    local version
    version=$(get_latest_version "$download_tool")
    info "Latest version: ${version}"

    # Check if already installed
    if command -v devdock >/dev/null 2>&1; then
        local current_version
        current_version=$(devdock --version 2>/dev/null | awk '{print $2}' || echo "unknown")
        warning "DevDock ${current_version} is already installed"

        if [ "$current_version" = "${version#v}" ]; then
            info "You already have the latest version installed"
            echo "" >&2
            read -p "Reinstall anyway? [y/N] " -n 1 -r
            echo "" >&2
            if [[ ! $REPLY =~ ^[Yy]$ ]]; then
                info "Installation cancelled"
                exit 0
            fi
        else
            info "Upgrading from ${current_version} to ${version#v}"
        fi
    fi

    # Download binary
    local temp_dir
    temp_dir=$(download_binary "$version" "$platform" "$download_tool")

    # Install binary
    install_binary "$temp_dir"

    # Verify installation
    if command -v devdock >/dev/null 2>&1; then
        local which_devdock
        which_devdock=$(which devdock)

        success "Installed to: ${INSTALL_DIR}/${BINARY_NAME}"

        # Check if using the correct binary location
        if [ "$which_devdock" != "${INSTALL_DIR}/${BINARY_NAME}" ]; then
            warning "Found existing devdock at: $which_devdock"
            info "This binary will take precedence (appears first in PATH)"
            echo "" >&2
            info "Overriding with new binary..."

            # Use install command for proper permissions
            if [ -w "$(dirname "$which_devdock")" ]; then
                install -m 755 "${INSTALL_DIR}/${BINARY_NAME}" "$which_devdock"
            else
                sudo install -m 755 "${INSTALL_DIR}/${BINARY_NAME}" "$which_devdock"
            fi

            # Remove quarantine on macOS
            if [ "$(uname -s)" = "Darwin" ]; then
                if [ -w "$(dirname "$which_devdock")" ]; then
                    xattr -d com.apple.quarantine "$which_devdock" 2>/dev/null || true
                else
                    sudo xattr -d com.apple.quarantine "$which_devdock" 2>/dev/null || true
                fi
            fi

            success "Replaced $which_devdock"
        fi

        # Final verification - test the binary actually runs
        local installed_version
        installed_version=$(devdock --version 2>/dev/null | awk '{print $2}')

        if [ -z "$installed_version" ]; then
            error "Binary installed but not working. Run: xattr -d com.apple.quarantine $(which devdock)"
        fi

        success "Active version: ${installed_version} ($(which devdock))"

        # Verify version matches what we expected
        if [ "$installed_version" != "${version#v}" ]; then
            warning "Installed version (${installed_version}) doesn't match expected (${version#v})"
            info "This may be due to PATH ordering. Check: which -a devdock"
        fi
    else
        check_path
    fi

    echo "" >&2
    echo "════════════════════════════════════════" >&2
    echo "" >&2
    success "Installation complete!"
    echo "" >&2
    info "Get started:"
    echo "  devdock --help" >&2
    echo "  devdock start <profile>" >&2
    echo "" >&2

    # Show troubleshooting tips if there are multiple devdock binaries
    if [ "$(which -a devdock 2>/dev/null | wc -l)" -gt 1 ]; then
        info "Multiple devdock binaries found in PATH:"
        which -a devdock | while read -r binary; do
            local ver
            ver=$("$binary" --version 2>/dev/null | awk '{print $2}' || echo "error")
            echo "  → $binary (v$ver)" >&2
        done
        echo "" >&2
        info "Using: $(which devdock)"
    fi
}

# Trap errors and provide helpful message
trap 'handle_error' ERR

handle_error() {
    echo "" >&2
    echo "════════════════════════════════════════" >&2
    error "Installation failed!"
    echo "" >&2
    echo "Common issues:" >&2
    echo "  • macOS Gatekeeper blocking: xattr -d com.apple.quarantine /usr/local/bin/devdock" >&2
    echo "  • Permission denied: Run with appropriate permissions" >&2
    echo "  • Network issues: Check GitHub connection" >&2
    echo "" >&2
    echo "For help: https://github.com/aryan-agrawal-glean/devdock/issues" >&2
}

main "$@"
