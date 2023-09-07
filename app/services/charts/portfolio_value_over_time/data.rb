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
        from daily_transaction_aggregates
        where bot_id = ? and status = 0
      ),
      windowed_data as (
        select
          created_at,
          sum(invested) over (order by created_at asc) as total_invested,
          sum(amount) over (order by created_at asc) as total_accumulated,
          rate
        from data
      )

      select
        created_at,
        total_invested,
        rate * total_accumulated as current_value,
        total_accumulated
      from windowed_data

      SQL
    end

    def present_data
      ->(row) { row.slice('created_at', 'total_invested', 'current_value').values }
    end
  end
end
