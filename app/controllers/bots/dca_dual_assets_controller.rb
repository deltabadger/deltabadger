class Bots::DcaDualAssetsController < Bots::Wizard::CreatesController
  private

  def bot_relation = current_user.bots.dca_dual_asset
end
