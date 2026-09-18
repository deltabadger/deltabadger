class Bots::DcaSingleAssets::AddApiKeysController < Bots::Wizard::AddApiKeysController
  include Bots::Wizard::Navigable

  private

  def current_step = :api
  def bot_relation = current_user.bots.dca_single_asset

  # This key step comes before any asset (exchange-first); once a basket is chosen the wizard runs in
  # the multi namespace, so the quote step is always the multi one.
  def step_path(key)
    case key
    when :currencies then new_bots_dca_single_assets_pick_buyable_asset_path
    when :exchange   then missing_exchange_path
    when :api        then new_bots_dca_single_assets_add_api_key_path
    when :spendable  then new_bots_dca_multi_assets_pick_spendable_asset_path
    end
  end

  # Nothing is chosen before this step, so there is no stock asset to send to the broker picker.
  def missing_exchange_path = new_bots_dca_single_assets_pick_exchange_path

  # Once a basket is chosen, its key step is the basket's (see PickExchangesController).
  def prerequisite_redirect_path = step_complete?(:assets) ? new_bots_dca_multi_assets_add_api_key_path : super

  # After (re-)validating the key, go to the first step still missing input: the asset step.
  def after_api_key_path = step_path(first_incomplete)
end
