# The changed position reader has no numerical vectors in ruby_vectors or page_figure_sources.
# Refresh only its source fingerprint; preserve existing encrypted vectors and their random IVs.
# Run the sync/MCP/page parity grids and the full suites after recording.
require 'digest'
require 'json'

source = 'app/models/exchanges/alpaca.rb'
hash = Digest::SHA256.file(Rails.root.join(source)).hexdigest
%w[ruby_vectors.json page_figure_sources.json].each do |name|
  path = Rails.root.join('rust/tests/fixtures', name)
  text = File.read(path)
  values = JSON.parse(text)
  old = name == 'ruby_vectors.json' ? values.fetch('ported_sources').fetch(source) : values.fetch(source)
  before = "#{source.to_json}: #{old.to_json}"
  raise "ambiguous fingerprint in #{name}" unless text.scan(before).one?

  File.write(path, text.sub(before, "#{source.to_json}: #{hash.to_json}"))
end

# The general MCP recorder predates control tools. Preserve its complete metadata,
# and re-record only the readers changed by this plan; full MCP parity checks behavior.
path = Rails.root.join('rust/src/web/mcp/metadata.json')
text = File.read(path)
values = JSON.parse(text)
%w[app/models/exchanges/alpaca.rb app/services/bot_api/orders/lookup.rb app/services/bot_api/bots/create_support.rb].each do |reader|
  old = values.fetch('sources').fetch(reader)
  current = Digest::SHA256.file(Rails.root.join(reader)).hexdigest
  before = "#{reader.to_json}: #{old.to_json}"
  raise "ambiguous MCP fingerprint for #{reader}" unless text.scan(before).one?

  text = text.sub(before, "#{reader.to_json}: #{current.to_json}")
end
File.write(path, text)
