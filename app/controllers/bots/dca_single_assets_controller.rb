class Bots::DcaSingleAssetsController < Bots::Wizard::CreatesController
  private

  def bot_relation = current_user.bots.dca_single_asset
end
