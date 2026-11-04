# Records Asset#infer_color_from_image and Utilities::Image for rust/tests/asset_colors.rs (jobs::asset_color): for each
# committed logo (rust/tests/fixtures/logos), served from CoinGecko's image host, the dominant colours and the pick; the
# hostile fixtures; the allow-list; and the method's guards (overrides, the deltabadger provider, a blank image_url).
#   bin/rails runner script/rust/record_asset_colors.rb rust/tests/fixtures/asset_colors.json
# Needs ImageMagick's `convert`, as the Rails image does; the version is recorded. Re-record when a pinned source moves
# (script/rust/repin).
require 'json'
require 'digest'
require 'webmock'

out = ARGV.fetch(0)
pinned = %w[app/models/utilities/image.rb app/models/asset.rb app/jobs/asset/infer_color_from_image_job.rb
            app/models/market_data_settings.rb]
sources = pinned.to_h { |p| [p, Digest::SHA256.file(Rails.root.join(p)).hexdigest] }

include WebMock::API
WebMock.enable!
WebMock.disable_net_connect!
requests = []
WebMock.after_request { |req, _| requests << req.uri.to_s }

dir = Rails.root.join('rust/tests/fixtures/logos')
host = 'https://coin-images.coingecko.com/coins/images/1/large/'
serve = ->(name) { stub_request(:get, host + name).to_return(status: 200, body: File.binread(dir.join(name)), headers: { 'Content-Type' => 'image/png' }) }

# bomb.png is Rust's own (D-COLOR-IMAGE-LIMITS): ImageMagick would rasterise its 20000x20000 pixels.
logos = Dir.children(dir).sort.reject { |n| n == 'bomb.png' }.map do |name|
  WebMock.reset!
  requests.clear
  serve.(name)
  colors = Utilities::Image.dominant_colors_from_url(host + name)
  { 'file' => name, 'coder' => Utilities::Image.coder_for(File.binread(dir.join(name))), 'colors' => colors,
    'pick' => colors.present? ? Utilities::Image.most_vivid_color(colors) : nil, 'requests' => requests.dup }
end

# A redirect whose body is an image, and a 404 whose body is one: Faraday follows nothing, and raise_error refuses 4xx/5xx.
statuses = [[302, { 'Location' => 'https://evil.com/x.png' }], [404, {}], [500, {}], [204, {}]].map do |status, headers|
  WebMock.reset!
  requests.clear
  stub_request(:get, "#{host}status.png").to_return(status:, body: File.binread(dir.join('bitcoin.png')), headers:)
  { 'status' => status, 'colors' => Utilities::Image.dominant_colors_from_url("#{host}status.png"), 'requests' => requests.dup }
end

urls = ['https://coin-images.coingecko.com/x.png', 'https://assets.coingecko.com/x.png', 'https://COIN-IMAGES.coingecko.com/x.png',
        'http://coin-images.coingecko.com/x.png', 'https://evil.com/x.png', 'https://coin-images.coingecko.com.evil.com/x.png',
        'https://evil.com@coin-images.coingecko.com/x.png', 'https://coin-images.coingecko.com@evil.com/x.png',
        'https://127.0.0.1/x.png', 'ftp://coin-images.coingecko.com/x.png', 'coin-images.coingecko.com/x.png', '', 'not a url',
        'https://data.deltabadger.com/logos/x.png', 'https://coin-images.coingecko.com:8443/x.png']
allowed = [nil, 'http://data-api:3000', 'https://data.example.com'].flat_map do |market_url|
  market_url ? AppConfig.market_data_url = market_url : AppConfig.where(key: AppConfig::MARKET_DATA_URL).delete_all
  urls.map { |u| { 'market_data_url' => market_url, 'url' => u, 'allowed' => Utilities::Image.allowed_source?(u) } }
end
AppConfig.where(key: AppConfig::MARKET_DATA_URL).delete_all

vivid = [%w[#000000 #FFFFFF], %w[#FFFFFF #000000], %w[#808080 #F7931A], ['#F7931A'], %w[#336699 #339966], %w[#E5E5E5 #1A1A1A],
         %w[#FF0000 #00FF00], []].map { |c| { 'colors' => c, 'pick' => Utilities::Image.most_vivid_color(c) } }

# The guards, each on a fresh asset whose image is bitcoin's.
WebMock.reset!
serve.('bitcoin.png')
guard = lambda do |label, external_id:, color:, provider:, image: (host + 'bitcoin.png')|
  AppConfig.market_data_provider = provider
  asset = Asset.create!(external_id:, symbol: 'X', name: 'X', category: 'Cryptocurrency', color:, image_url: image)
  requests.clear
  asset.infer_color_from_image
  row = { 'case' => label, 'external_id' => external_id, 'color_before' => color, 'provider' => provider, 'image_url' => image,
          'color_after' => asset.reload.color, 'requests' => requests.size }
  row
end
guards = [
  guard.('coingecko_new', external_id: 'g-new', color: nil, provider: 'coingecko'),
  guard.('coingecko_recolour', external_id: 'g-old', color: '#123456', provider: 'coingecko'),
  guard.('deltabadger_colour_kept', external_id: 'g-db', color: '#123456', provider: 'deltabadger'),
  guard.('deltabadger_no_colour', external_id: 'g-db2', color: nil, provider: 'deltabadger'),
  guard.('override', external_id: 'ripple', color: nil, provider: 'coingecko'),
  guard.('override_deltabadger_colour', external_id: 'altlayer', color: '#123456', provider: 'deltabadger'),
  guard.('blank_image', external_id: 'g-blank', color: '#123456', provider: 'coingecko', image: nil),
  guard.('disallowed_host', external_id: 'g-host', color: '#123456', provider: 'coingecko', image: 'https://evil.com/x.png')
]

File.write(out, "#{JSON.pretty_generate({ 'sources' => sources, 'imagemagick' => `convert -version`.lines.first.to_s.strip,
                                           'overrides' => Asset::COLOR_OVERRIDES, 'logos' => logos, 'allowed' => allowed,
                                           'vivid' => vivid, 'guards' => guards, 'statuses' => statuses })}\n")
