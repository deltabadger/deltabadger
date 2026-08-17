# frozen_string_literal: true

Rails.application.config.version = begin
  cargo_toml_path = Rails.root.join('src-tauri', 'Cargo.toml')
  if File.exist?(cargo_toml_path)
    content = File.read(cargo_toml_path)
    match = content.match(/^version\s*=\s*"([^"]+)"/)
    Rails.logger.warn "Application version unavailable: #{cargo_toml_path} has no package version; using 0.0.0" unless match
    match ? match[1] : '0.0.0'
  else
    Rails.logger.warn "Application version unavailable: #{cargo_toml_path} does not exist; using 0.0.0"
    '0.0.0'
  end
end

Rails.application.config.action_mcp.version = Rails.application.config.version
