class BotsRepository < BaseRepository
  SYMBOLS_HASH = {
    'XDG': 'DOGE',
    'XBT': 'BTC'
  }.freeze

  TOP_BOTS_KEY = 'TOP_BOTS_CACHE_KEY'.freeze

  def by_id_for_user(user, id)
    user.bots.without_deleted.find(id)
  end

  def for_user(user)
    user
      .bots
      .without_deleted
      .includes(:exchange, :transactions)
      .all
  end

  def count_with_status(status)
    model
      .where(status: status)
      .count
  end

  def list_top_ten
    most_popular_bots(10)
  end

  def model
    Bot
  end

  def top_ten_bots
    if Rails.cache.exist?(TOP_BOTS_KEY)
      Rails.cache.read(TOP_BOTS_KEY).empty? ? Rails.cache.read(TOP_BOTS_KEY) : top_bots_update
    else
      top_bots_update
    end
  end

  def top_bots_update
    top_ten_bots = list_top_ten
    top_bots_array = top_ten_bots.map do |key, value|
      {
        name: key,
        counter: value,
        is_up: false
      }
    end
    old_top_bots = Rails.cache.exist?(TOP_BOTS_KEY) ? Rails.cache.read(TOP_BOTS_KEY) : top_bots_array
    top_bots_array.each_with_index do |new_bot, new_index|
      found = false
      old_index = old_top_bots.index { |bot| bot[:name] == new_bot[:name] }
      unless old_index.nil?
        found = true
        new_bot[:is_up] = true if new_index < old_index
      end
      new_bot[:is_up] = true unless found
    end
    Rails.cache.write(TOP_BOTS_KEY, top_bots_array, expires_in: 25.hour)
    top_bots_array
  end

  private

  def most_popular_bots(amount)
    all_bots_hash = Bot.group("bots.settings->>'base'")
                       .order(count: :desc)
                       .where(status: 'working')
                       .count
    fetched_symbols_bots_hash = all_bots_hash.each do |key, value|
      SYMBOLS_HASH.each do |symbol_key, symbol_value|
        if key == symbol_key.to_s
          all_bots_hash[symbol_value.to_s] += value
          all_bots_hash.delete(symbol_key.to_s)
        end
      end
    end
    fetched_symbols_bots_hash.sort_by { |_key, value| -value }[0..amount - 1]
  end

end
