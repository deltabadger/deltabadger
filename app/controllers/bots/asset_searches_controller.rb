class Bots::AssetSearchesController < ApplicationController
  include Bots::Botable
  include Bots::Searchable

  before_action :authenticate_user!
  before_action :set_bot

  def edit
    asset_type = search_params[:asset_field] == 'quote_asset_id' ? :quote_asset : :base_asset
    @assets = asset_search_results(@bot, search_params[:query], asset_type)
    @asset_field = search_params[:asset_field]
  end

  private

  def search_params
    params.permit(:query, :asset_field)
  end
end
