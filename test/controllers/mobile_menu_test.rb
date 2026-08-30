# frozen_string_literal: true

require 'test_helper'

# The mobile drawer. It is held open by a turbo-permanent checkbox, so nothing closes it on its own
# — a Turbo visit carries the open state onto the page you just picked.
class MobileMenuTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true) # satisfies the onboarding gate (an admin must exist)
    sign_in create(:user)
  end

  test 'picking a destination closes the drawer' do
    get bots_path

    assert_select '.menu-mobile[data-controller=menu-mobile]'
    assert_select '.menu-mobile[data-action="click->menu-mobile#close"]'
  end
end
