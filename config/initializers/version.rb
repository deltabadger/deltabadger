# frozen_string_literal: true

Rails.application.config.version = begin
  cargo_toml_path = Rails.root.join("src-tauri", "Cargo.toml")
  if File.exist?(cargo_toml_path)
    content = File.read(cargo_toml_path)
    match = content.match(/^version\s*=\s*"([^"]+)"/)
    match ? match[1] : "0.0.0"
  else
    "0.0.0"
  end
end
