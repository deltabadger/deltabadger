module Charts::PortfolioValueOverTime
  class Data < BaseService
    def call(bot)
      sanitized_sql = ActiveRecord::Base.sanitize_sql([query, bot.id])
      response = ActiveRecord::Base.connection.execute(sanitized_sql)
      response.map(&present_data)
    end

    private

    def query
      <<~SQL
        with data as (
          select
            created_at,
            rate * amount as invested,
            rate,
            amount
          from transactions
          where bot_id = ?
        )

        select
          created_at,
          total_invested,
          rate * total_accumulated as current_value,
          total_accumulated
          from (
            SELECT
              created_at,
              sum(invested) over (order by created_at asc rows between unbounded preceding and current row) as total_invested,
              sum(amount) over (order by created_at asc rows between unbounded preceding and current row) as total_accumulated,
              rate
            from data) t1
      SQL
    end

    private

    def present_data
      ->(row) { row.slice('created_at', 'total_invested', 'current_value').values }
    end
  end
end
