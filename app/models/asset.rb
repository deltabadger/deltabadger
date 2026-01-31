class Asset < ApplicationRecord
  has_many :exchange_assets
  has_many :exchanges, through: :exchange_assets

  validates :external_id, presence: true, uniqueness: true
  validate :can_be_destroyed, on: :destroy

  include Undeletable

  # https://docs.coingecko.com/reference/simple-supported-currencies
  VS_CURRENCIES = %w[usd eur jpy gbp cad aud chf btc].freeze
  COINGECKO_BLACKLISTED_IDS = [
    'covalent', # TODO: remove this once covalent is supported in coingecko
    'assister-ai' # TODO: remove this once assister-ai is supported in coingecko
  ].freeze

  # Manual color overrides for assets where image extraction fails or produces poor results
  # Keys are CoinGecko external_ids, not ticker symbols
  COLOR_OVERRIDES = {
    'ripple' => '#6366F1',
    'altlayer' => '#6f51b1',
    'nervos-network' => '#3cc68a',
    'lisk' => '#3e6ded',
    'bittensor' => '#00C48C',
    'algorand' => '#00bea5',
    'arweave' => '#ff6700',
    'stellar' => '#04b5e5',
    'artificial-superintelligence-alliance' => '#4B0082',
    'quant' => '#bd2426',
    'worldcoin' => '#1A1A1A',
    'forefront' => '#6366F1',
    'ai-rig-complex' => '#001f3f',
    'jito' => '#085639',
    'convex-finance' => '#1682fe',
    'big-time' => '#FFD700',
    'axelar' => '#ff6414',
    'syndicate' => '#5865F2'
  }.freeze

  def sync_data_with_coingecko(prefetched_data: nil)
    return Result::Success.new(self) unless AppConfig.coingecko_configured?
    return Result::Success.new(self) if COINGECKO_BLACKLISTED_IDS.include?(external_id)

    data = prefetched_data || begin
      result = coingecko.get_coin_data_by_id(coin_id: external_id)
      return result if result.failure?

      result.data
    end

    update!(
      symbol: Utilities::Hash.dig_or_raise(data, 'symbol').upcase,
      name: Utilities::Hash.dig_or_raise(data, 'name'),
      url: "https://www.coingecko.com/coins/#{data['web_slug'] || external_id}",
      image_url: Utilities::Hash.safe_dig(data, 'image', 'large') || data['image'],
      market_cap_rank: data['market_cap_rank'],
      market_cap: Utilities::Hash.safe_dig(data, 'market_data', 'market_cap', 'usd') || data['market_cap'],
      circulating_supply: Utilities::Hash.safe_dig(data, 'market_data', 'circulating_supply') || data['circulating_supply']
    )
    Result::Success.new(self)
  end

  def infer_color_from_image
    if COLOR_OVERRIDES.key?(external_id)
      update!(color: COLOR_OVERRIDES[external_id])
      return
    end

    return if image_url.blank?

    # some images have single quotes in the url that ImageMagick doesn't like
    parsed_image_url = image_url.gsub("'", '%27')

    colors = Utilities::Image.extract_dominant_colors(parsed_image_url)
    update!(color: Utilities::Image.most_vivid_color(colors))
  end

  def get_price(currency: 'usd')
    return Result::Failure.new('Asset is not a cryptocurrency') if category != 'Cryptocurrency'

    result = coingecko.get_price(coin_id: external_id, currency: currency)
    return result if result.failure?

    Result::Success.new(result.data)
  end

  def get_market_cap(currency: 'usd')
    return Result::Failure.new('Asset is not a cryptocurrency') if category != 'Cryptocurrency'

    result = coingecko.get_market_cap(coin_id: external_id, currency: currency)
    return result if result.failure?

    Result::Success.new(result.data)
  end

  private

  def coingecko
    @coingecko ||= Coingecko.new(api_key: AppConfig.coingecko_api_key)
  end
end
