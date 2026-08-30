# frozen_string_literal: true

require 'test_helper'

# The desktop menu's highlight is one chip that slides between the tiles. The slide is done in the
# browser, but it matches the incoming page's active tile by `data-tile`, and the chip is placed by
# CSS from that same class — so every signed-in page has to render the section the controller
# expects: the chip, and at most ONE active tile, each tile named.
class MenuChipTest < ActionDispatch::IntegrationTest
  MENU = '.menu__section--main-menu[data-controller=menu]'

  setup do
    create(:user, admin: true) # satisfies the onboarding gate (an admin must exist)
    @user = create(:user)
    sign_in @user
  end

  { bots: :bots_path, tracker: :tracker_path, rules: :rules_path }.each do |tile, route|
    test "the #{tile} page lights the #{tile} tile alone" do
      get send(route)

      assert_menu_with_active tile
    end
  end

  test 'a single-bot account lights the bots tile on that bot' do
    bot = create(:dca_single_asset, user: @user)

    get bot_path(id: bot.id)

    assert_menu_with_active :bots
  end

  test 'a page with no tile of its own still renders the chip, unlit' do
    get settings_account_path

    assert_select MENU do
      assert_select '.menu__chip[data-menu-target=chip]', 1
      assert_select '.menu__item--active', 0
    end
  end

  private

  def assert_menu_with_active(tile)
    assert_select MENU do
      assert_select '.menu__chip[data-menu-target=chip]', 1
      assert_select '.menu__item[data-menu-target=tile][data-tile]', 3
      assert_select '.menu__item--active', 1
      assert_select ".menu__item--active[data-tile=#{tile}]", 1
    end
  end
end
