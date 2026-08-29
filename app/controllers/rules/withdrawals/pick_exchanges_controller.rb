class Rules::Withdrawals::PickExchangesController < ApplicationController
  before_action :authenticate_user!

  include Bots::Searchable

  def new
    session[:withdrawal_rule_config] = {}
    # A temporary bot, for the exchange list (and its stable-first order) the bot wizard draws.
    @bot = current_user.bots.dca_single_asset.new
    @exchanges = exchange_search_results(@bot, nil).select(&:supports_withdrawal?)
  end

  def create
    exchange_id = params.dig(:bots_dca_single_asset, :exchange_id)
    if exchange_id.present?
      session[:withdrawal_rule_config] ||= {}
      session[:withdrawal_rule_config]['exchange_id'] = exchange_id
      session[:withdrawal_rule_config].delete('asset_id')
      redirect_to new_rules_withdrawals_pick_asset_path
    else
      redirect_to new_rules_withdrawals_pick_exchange_path
    end
  end
end
