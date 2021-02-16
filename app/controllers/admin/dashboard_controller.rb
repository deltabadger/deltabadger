class Admin::DashboardController < Admin::ApplicationController
  def index
    transations_by_exchange = Exchange.all.map(&present_exchange)

    render :index, locals: {
      number_of_all_users: User.count,
      number_of_investor_plans: SubscriptionsRepository.new.all_current_count('investor'),
      number_of_hodler_plans: SubscriptionsRepository.new.all_current_count('hodler'),
      number_of_all_bots: Bot.count,
      number_of_working_bots: BotsRepository.new.count_with_status('working'),
      number_of_all_transactions: Transaction.count,
      btc_amount_bought: TransactionsRepository.new.total_btc_bought,
      btc_amount_sold: TransactionsRepository.new.total_btc_sold,
      transations_by_exchange: transations_by_exchange,
      payments_statistics: CalculateSalesStatistics.new.call
    }
  end

  def model_name
    :dashboard
  end

  private

  ExchangeTransactions = Struct.new(
    :name,
    :successful_transactions,
    :failed_transactions,
    :total
  )

  def present_exchange
    lambda do |exchange|
      transactions_repository = TransactionsRepository.new
      successful = transactions_repository.count_by_status_and_exchange(:success, exchange)
      failed = transactions_repository.count_by_status_and_exchange(:failure, exchange)

      ExchangeTransactions.new(
        exchange.name.capitalize,
        successful,
        failed,
        successful + failed
      )
    end
  end
end
