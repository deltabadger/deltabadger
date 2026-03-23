class Tracker::PickExchangesController < ApplicationController
  before_action :authenticate_user!

  include Bots::Searchable

  def new
    @exchanges = filter_exchanges_by_query(exchanges: available_exchanges, query: search_params[:query])
  end

  def create
    exchange = Exchange.find(params[:exchange_id])
    session[:tracker_connect] = { 'exchange_id' => exchange.id }
    redirect_to new_tracker_add_api_key_path
  end

  private

  def search_params
    params.permit(:query)
  end

  def available_exchanges
    Exchange.available
  end
end
