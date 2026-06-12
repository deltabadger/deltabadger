class Bots::DcaIndexesController < Bots::Wizard::CreatesController
  private

  def bot_relation = current_user.bots.dca_index
end
