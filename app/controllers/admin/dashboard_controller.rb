class Admin::DashboardController < Admin::ApplicationController
  def index
    transations_by_exchange = Exchange.all.map(&present_exchange)

    render :index, locals: {
      number_of_all_users: User.count,
      number_of_unlimited_plans: SubscriptionsRepository.new.all_current_unlimited_count,
      number_of_all_bots: Bot.count,
      number_of_all_transactions: Transaction.count,
      transations_by_exchange: transations_by_exchange
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
