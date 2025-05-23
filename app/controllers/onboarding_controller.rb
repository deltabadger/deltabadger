class OnboardingController < ApplicationController
  before_action :authenticate_user!
  before_action :check_onboarding_status

  layout 'onboarding'

  def step1
    # First step - Investment goal
  end

  def step2
    # Second step - Preferred exchange
  end

  def save_step1
    if current_user.update(investment_goal: params[:investment_goal])
      redirect_to onboarding_step2_path
    else
      flash.now[:alert] = current_user.errors.full_messages.to_sentence
      render :step1
    end
  end

  def save_step2
    # Join the selected exchanges into a comma-separated string
    selected_exchanges = params[:preferred_exchange]

    if selected_exchanges.present?
      if current_user.update(preferred_exchange: selected_exchanges.join(','), onboarding_completed: true)
        redirect_to bots_path
      else
        flash.now[:alert] = current_user.errors.full_messages.to_sentence
        render :step2
      end
    else
      flash.now[:alert] = 'Please select at least one exchange'
      render :step2
    end
  end

  private

  def check_onboarding_status
    redirect_to bots_path if current_user.onboarding_complete?
  end
end
