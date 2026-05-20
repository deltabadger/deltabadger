# Merges a bot's Transactions and BotActivityLogs into one newest-first feed with
# cursor-based pagination. View-free: returns ordered records and an opaque cursor.
#
# Feed order: created_at desc, kind asc (activity before transaction), id desc.
# Each table is fetched with the full tuple-cursor predicate (so a cluster of rows
# sharing one created_at can't be truncated/skipped), then merged in Ruby (no SQL
# UNION) — fine while page size is small and the two row types render differently.
class BotActivityFeed
  KIND_RANK = { 'activity' => 0, 'transaction' => 1 }.freeze
  # Already represented by a skipped Transaction row, or a rare no-op. (execution_failed
  # is kept: it also covers failures without a transaction, e.g. auth/market errors.)
  EXCLUDED_EVENTS = %w[order_skipped order_ignored].freeze

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
      candidates = fetch(@bot.transactions, 'transaction') +
                   fetch(@bot.bot_activity_logs.where.not(event: EXCLUDED_EVENTS), 'activity')
      ordered = candidates.sort_by { |record| sort_key(record) }.reverse
      page = ordered.first(@limit)
      next_cursor = ordered.size > @limit ? cursor_for(page.last)&.to_param : nil
      { items: page, next_cursor: next_cursor }
    end
  end

  # Rows strictly "after" the cursor in feed order, scoped per table so a same-timestamp
  # cluster is never cut off. limit+1 from each table is enough for a correct merge of
  # the top `limit` plus has-more detection.
  def fetch(relation, kind)
    relation = relation.where(*cursor_predicate(kind)) if @cursor
    relation.order(created_at: :desc, id: :desc).limit(@limit + 1).to_a
  end

  def cursor_predicate(kind)
    cursor_time = @cursor.created_at
    if kind == @cursor.kind
      ['created_at < :t OR (created_at = :t AND id < :id)', { t: cursor_time, id: @cursor.id }]
    elsif KIND_RANK.fetch(kind) > KIND_RANK.fetch(@cursor.kind)
      # this kind sorts after the cursor's kind at an equal timestamp -> include those too
      ['created_at <= ?', cursor_time]
    else
      ['created_at < ?', cursor_time]
    end
  end

  def sort_key(record)
    [record.created_at, -KIND_RANK.fetch(kind_of(record)), record.id]
  end

  def kind_of(record)
    record.is_a?(Transaction) ? 'transaction' : 'activity'
  end

  def cursor_for(record)
    return nil if record.nil?

    Cursor.new(record.created_at, kind_of(record), record.id)
  end
end
