# Shared "pick a stock broker" wizard step (single/dual). Stock bots route
# through this step instead of the crypto exchange picker — see
# Bots::StockBrokerRoutable for the auto-skip rules. Subclasses supply the bot
# relation, routes, params and the stock test as explicit overrides.
class Bots::Wizard::PickStockBrokersController < ApplicationController
  before_action :authenticate_user!

  include Bots::StockBrokerRoutable

  def new
    @bot = build_bot
    return redirect_to repick_asset_path unless stock_bot?

    @brokers = available_stock_brokers(@bot)
    redirect_to repick_asset_path if @brokers.empty?
  end

  def create
    @bot = build_bot
    return redirect_to repick_asset_path unless stock_bot?

    @brokers = available_stock_brokers(@bot)
    chosen = @brokers.find { |exchange| exchange.id.to_s == bot_params[:exchange_id].to_s }
    if chosen
      finalize_stock_broker!(chosen)
      redirect_to add_api_key_path
    else
      render :new, status: :unprocessable_entity
    end
  end

  private

  def bot_relation
    raise NotImplementedError
  end

  def build_bot = bot_relation.new(sanitized_bot_config)
end
