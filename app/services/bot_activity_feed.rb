# Merges a bot's Transactions and BotActivityLogs into one newest-first feed with
# cursor-based pagination. View-free: returns ordered records and an opaque cursor.
#
# Feed order: created_at desc, kind asc (activity before transaction), id desc.
# Pagination fetches a small window from each table around the cursor and merges in
# Ruby (no SQL UNION) — fine while page size is small and the two rows render
# differently.
class BotActivityFeed
  KIND_RANK = { 'activity' => 0, 'transaction' => 1 }.freeze
  FETCH_BUFFER = 5
  # Already represented by a Transaction row (skipped/failed) or a rare no-op, so they
  # are kept out of the user-facing feed to avoid duplicate entries.
  EXCLUDED_EVENTS = %w[order_skipped order_ignored execution_failed].freeze

  Cursor = Struct.new(:created_at, :kind, :id) do
    def self.parse(value)
      return nil if value.blank?

      created_at, kind, id = value.split('|', 3)
      return nil unless KIND_RANK.key?(kind)

      new(Time.iso8601(created_at), kind, id.to_i)
    rescue ArgumentError, TypeError
      nil
    end

    def to_param
      "#{created_at.utc.iso8601(6)}|#{kind}|#{id}"
    end
  end

  def initialize(bot:, before: nil, limit: 10)
    @bot = bot
    @cursor = Cursor.parse(before)
    @limit = limit
  end

  def items
    load[:items]
  end

  def next_cursor
    load[:next_cursor]
  end

  private

  def load
    @load ||= begin
      candidates = fetch(@bot.transactions) + fetch(@bot.bot_activity_logs.where.not(event: EXCLUDED_EVENTS))
      ordered = candidates.sort_by { |record| sort_key(record) }.reverse
      ordered = ordered.select { |record| (sort_key(record) <=> cursor_key) == -1 } if @cursor
      page = ordered.first(@limit)
      next_cursor = ordered.size > @limit ? cursor_for(page.last)&.to_param : nil
      { items: page, next_cursor: next_cursor }
    end
  end

  def fetch(relation)
    relation = relation.where('created_at <= ?', @cursor.created_at) if @cursor
    relation.order(created_at: :desc, id: :desc).limit(@limit + FETCH_BUFFER).to_a
  end

  def sort_key(record)
    [record.created_at, -KIND_RANK.fetch(kind_of(record)), record.id]
  end

  def cursor_key
    [@cursor.created_at, -KIND_RANK.fetch(@cursor.kind), @cursor.id]
  end

  def kind_of(record)
    record.is_a?(Transaction) ? 'transaction' : 'activity'
  end

  def cursor_for(record)
    return nil if record.nil?

    Cursor.new(record.created_at, kind_of(record), record.id)
  end
end
