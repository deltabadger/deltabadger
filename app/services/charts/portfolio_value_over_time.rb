module Charts
  class PortfolioValueOverTime < BaseService
    def call(bot) # rubocop:disable Metrics/MethodLength
      query = <<~SQL
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
          rate * total_accumulated as current_value
          from (
            SELECT
              created_at,
              sum(invested) over (order by created_at asc rows between unbounded preceding and current row) as total_invested,
              sum(amount) over (order by created_at asc rows between unbounded preceding and current row) as total_accumulated,
              rate
            from data) t1
      SQL

      sanitized_sql = ActiveRecord::Base.sanitize_sql([query, bot.id])
      response = ActiveRecord::Base.connection.execute(sanitized_sql)

      date = response.map { |el| el.fetch('created_at') }

      total_invested = response.map { |el| el.fetch('total_invested') }
      value = response.map { |el| el.fetch('current_value') }

      output = date.zip(total_invested, value)

      Result::Success.new(output)
    end
  end
end
