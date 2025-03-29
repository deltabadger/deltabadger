class Asset < ApplicationRecord
  has_many :exchange_assets
  has_many :exchanges, through: :exchange_assets

  validates :external_id, presence: true, uniqueness: true
  validate :can_be_destroyed, on: :destroy

  include Undeletable

  def sync_data_with_coingecko
    result = coingecko_client.coin_data_by_id(id: external_id)
    return Result::Failure.new("Failed to get #{external_id} data from coingecko") unless result.success?

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

    colors = Utilities::Image.extract_dominant_colors(image_url)
    update!(color: Utilities::Image.most_vivid_color(colors))
  end

  private

  def coingecko_client
    @coingecko_client ||= CoingeckoClient.new
  end
end
