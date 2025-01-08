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
        with daily_data as (
          select distinct on (date_trunc('day', created_at))
            date_trunc('day', created_at) as day,
            created_at,
            rate * amount as invested,
            amount,
            rate
          from daily_transaction_aggregates
          where bot_id = ? and status = 0
          order by date_trunc('day', created_at), created_at desc
        ),
        windowed_data as (
          select
            day as created_at,
            sum(invested) over (order by day asc) as total_invested,
            sum(amount) over (order by day asc) as total_accumulated,
            rate
          from daily_data
        )

        select
          created_at,
          total_invested,
          rate * total_accumulated as current_value,
          total_accumulated
        from windowed_data
        order by created_at asc
      SQL
    end

    def present_data
      ->(row) { row.slice('created_at', 'total_invested', 'current_value').values }
    end
  end
end
