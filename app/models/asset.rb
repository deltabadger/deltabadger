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
    'fetch-ai' => '#4B0082',
    'quant' => '#bd2426',
    'worldcoin-wld' => '#1A1A1A',
    'forefront' => '#6366F1',
    'ai-rig-complex' => '#001f3f',
    'jito' => '#085639',
    'convex-finance' => '#1682fe',
    'big-time' => '#FFD700',
    'axelar' => '#ff6414',
    'syndicate' => '#5865F2',
    'zksync' => '#1a24b2',
    'immutable-x' => '#ecc968',
    'falcon-finance' => '#0EA5E9',
    'usdtb' => '#1E3A5F',
    'apenft' => '#151432',
    'pippin' => '#5B8DEF',
    'falcon-finance-ff' => '#0EA5E9',
    'aioz-network' => '#35D687',
    'aleo' => '#c4ffc2',
    'creditcoin-2' => '#1EEEB0',
    'moonbirds' => '#39438A',
    'centrifuge-2' => '#FFC012',
    'ergo' => '#E74C3C',
    'wojak-4' => '#90FF4A',
    'boundless' => '#537263',
    'avici' => '#FFA727',
    'covalent' => '#FF4C8B',
    'future-ai' => '#3380F6',
    'onomy-protocol' => '#353340',
    'litr' => '#7EB4D4',
    'infiblue-world' => '#3A80A7',
    'smell' => '#E86CA0',
    'qitmeer-network' => '#58E7A3',
    'sakura-united-platform' => '#C22A2A',
    'crystal-palace-fan-token' => '#1B458F',
    'neblio' => '#50479E',
    'calamari-network' => '#6705BA',
    'lumishare' => '#D4AF37',
    'safe-road-club' => '#44A1B0',
    'pmg-coin' => '#E84460',
    'donablock' => '#4CAF82',
    'soldex' => '#9C1CD8',
    'secretum' => '#B8C4E6',
    'leeds-united-fan-token' => '#1D428A',
    'futurecoin' => '#7B2D8E',
    'snkrz-fit' => '#FF6B2B',
    'xpmarket' => '#1A1F71',
    'privateai' => '#6C3FC5',
    'yachtingverse-old' => '#1565C0',
    'marvellex-classic' => '#D4A017',
    'blend-3' => '#4A90D9',
    'dragon-3' => '#C62828',
    'nomoex-token' => '#2962FF',
    'qlindo' => '#2E7D32',
    'zynecoin' => '#F5A623',
    'virtual-x' => '#00BFA5',
    'evrynet' => '#3D5AFE',
    'wodo-gaming' => '#7C4DFF',
    'apollo-name-service' => '#FF7043',
    'consciousdao' => '#00ACC1',
    'storepay' => '#5C6BC0',
    'mongol-nft' => '#E53935',
    'selo' => '#4A6CF7',
    'fimarkcoin-com' => '#1E88E5',
    'crt-ai-network' => '#6C5CE7',
    'sedra-coin' => '#00B4D8',
    'dank-doge' => '#E8A317',
    'arkefi' => '#C9A84C',
    'yolo' => '#FF4136',
    'waltonchain' => '#8247E5',
    'eon-marketplace' => '#5B6EE1',
    'wattton' => '#4CAF50',
    'lightning-bitcoin' => '#F7931A',
    'inofi' => '#7B61FF',
    'conun' => '#2196F3',
    'sportsology-game' => '#E53935',
    'artube' => '#FF6B6B',
    'talki' => '#3B82F6',
    'bionergy' => '#2ECC71'
  }.freeze

  def sync_data_with_coingecko(prefetched_data: nil)
    return Result::Success.new(self) unless MarketData.configured?
    return Result::Success.new(self) if COINGECKO_BLACKLISTED_IDS.include?(external_id)

    data = prefetched_data || begin
      return Result::Failure.new('CoinGecko API key not configured') unless AppConfig.coingecko_configured?

      result = MarketData.coingecko.get_coin_data_by_id(coin_id: external_id)
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
    return if color.present? && MarketDataSettings.deltabadger?

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
    return Result::Failure.new('No market data provider configured') unless MarketData.configured?

    MarketData.get_price(coin_id: external_id, currency: currency)
  end
end
