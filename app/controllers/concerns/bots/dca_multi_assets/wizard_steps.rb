module Bots::DcaMultiAssets::WizardSteps
  extend ActiveSupport::Concern
  include Bots::Wizard::Navigable

  private

  def bot_relation = current_user.bots.dca_multi_asset
  def add_api_key_path = new_bots_dca_multi_assets_add_api_key_path
  def wizard_default_settings = Bots::WizardDefaults::MULTI

  # :exchange resolves through the stock-aware route so a stock basket with no broker does not
  # fall through to the crypto picker.
  def step_path(key)
    case key
    when :assets    then new_bots_dca_multi_assets_pick_assets_path
    when :exchange  then missing_exchange_path
    when :api       then new_bots_dca_multi_assets_add_api_key_path
    when :spendable then new_bots_dca_multi_assets_pick_spendable_asset_path
    end
  end

  def chosen_asset_ids = Array(session.dig(:bot_config, 'settings', 'base_asset_ids')).map(&:to_i)

  def stock_bot? = Asset.where(id: chosen_asset_ids, category: 'Stock').exists?

  def missing_exchange_path
    stock_bot? ? new_bots_dca_multi_assets_pick_stock_broker_path : new_bots_dca_multi_assets_pick_exchange_path
  end

  def repick_asset_path = new_bots_dca_multi_assets_pick_assets_path

  # Re-validating a key skips any answers that are still complete after the exchange changes.
  def after_api_key_path = step_path(first_incomplete)
end
