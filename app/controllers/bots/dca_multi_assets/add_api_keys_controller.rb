class Bots::DcaMultiAssets::AddApiKeysController < Bots::Wizard::AddApiKeysController
  include Bots::DcaMultiAssets::WizardSteps

  private

  def current_step = :api
end
