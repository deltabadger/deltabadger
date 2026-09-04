class Tracker::PickExchangesController < ApplicationController
  before_action :authenticate_user!

  include Bots::Searchable

  def new
    # No query: the grid is the picker, `nil` only buys the stable-first ordering.
    @exchanges = filter_exchanges_by_query(exchanges: available_exchanges, query: nil)
  end

  def create
    exchange = Exchange.find(params[:exchange_id])
    session[:tracker_connect] = { 'exchange_id' => exchange.id }
    redirect_to new_tracker_add_api_key_path
  end

  private

  def available_exchanges
    Exchange.available
  end
end
