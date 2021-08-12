class BotsRepository < BaseRepository
  SYMBOLS_HASH = {
    'XDG': 'DOGE',
    'XBT': 'BTC'
  }.freeze
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

  private

  def most_popular_bots(amount)
    all_bots_hash = Bot.group("bots.settings->>'base'")
                       .order(count: :desc)
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
