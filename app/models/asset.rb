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

  def sync_data_with_coingecko
    return Result::Success.new(self) if COINGECKO_BLACKLISTED_IDS.include?(external_id)

    result = coingecko.get_coin_data_by_id(coin_id: external_id)
    return result if result.failure?

    update!(
      symbol: Utilities::Hash.dig_or_raise(result.data, 'symbol').upcase,
      name: Utilities::Hash.dig_or_raise(result.data, 'name'),
      url: "https://www.coingecko.com/coins/#{Utilities::Hash.dig_or_raise(result.data, 'web_slug')}",
      image_url: Utilities::Hash.dig_or_raise(result.data, 'image', 'large'),
      market_cap_rank: result.data['market_cap_rank']
    )
    Result::Success.new(self)
  end

  def infer_color_from_image
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
    @coingecko ||= Coingecko.new
  end
end
