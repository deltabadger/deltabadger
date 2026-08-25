# frozen_string_literal: true

require 'test_helper'

# The segmented control: one track, one sliding chip, one option selected.
#
# Two sizings, because two different jobs. EQUAL splits the track into identical columns —
# right when the labels are the same length and a jumping chip width would read as a glitch
# (VALUE / RETURN). FLUID lets every option take the width of its own label and the chip
# follows, which is the only honest option when the labels differ ("All" against
# "Transactions") or when the number of options depends on the data.
#
# Navigation and selection are different controls and are marked up differently: options that
# carry an href are links (aria-current), options that do not are radios in a radiogroup.
class SegmentedTest < ActionView::TestCase
  def render_segmented(**options)
    render partial: 'shared/segmented', locals: { label: 'Mode' }.merge(options)
  end

  def buttons(value: 'a')
    [{ value: 'a', label: 'Alpha', active: value == 'a' },
     { value: 'b', label: 'Beta', active: value == 'b' }]
  end

  test 'the track carries one chip and one option per entry' do
    render_segmented(options: buttons)

    assert_select '.segmented', 1
    assert_select '.segmented__thumb', 1
    assert_select '.segmented__option', 2
  end

  test 'equal is the default sizing and fluid is opt-in' do
    render_segmented(options: buttons)
    assert_select '.segmented--fluid', 0

    render_segmented(options: buttons, fluid: true)
    assert_select '.segmented--fluid', 1
  end

  test 'options without a link are radios, and only the selected one is in the tab order' do
    render_segmented(options: buttons(value: 'b'))

    assert_select '[role="radiogroup"]', 1
    assert_select 'button[role="radio"]', 2
    assert_select 'button[data-value="b"][aria-checked="true"][tabindex="0"].is-on', 1
    assert_select 'button[data-value="a"][aria-checked="false"][tabindex="-1"]', 1
  end

  test 'options with a link are navigation, not a radiogroup' do
    render_segmented(options: [{ value: 'all', label: 'All', href: '/bots/all', active: true },
                               { value: 'active', label: 'Active', href: '/bots/active' }])

    assert_select '[role="radiogroup"]', 0
    assert_select 'a.segmented__option', 2
    assert_select 'a[href="/bots/all"][aria-current="page"].is-on', 1
    assert_select 'a[href="/bots/active"][aria-current]', 0
  end

  # Without this the chip only ever teleports: the click leaves the page and the next one places
  # its chip from scratch. Arrow keys stay off a link group — those are tabbed through.
  test 'a link still moves the chip on the way out, so the switch animates' do
    render_segmented(options: [{ value: 'all', label: 'All', href: '/bots/all', active: true }])

    assert_select 'a[data-action="segmented#select"]', 1
  end

  test 'the label names the group for a screen reader' do
    render_segmented(options: buttons, label: 'Chart mode')

    assert_select '[aria-label="Chart mode"]', 1
  end

  test 'per-option data rides along, so a consumer controller can still read its own attributes' do
    render_segmented(options: [{ value: 'all', label: 'All', active: true,
                                 data: { order_filter_target: 'filter', filter_type: 'all' } }])

    assert_select '[data-order-filter-target="filter"][data-filter-type="all"]', 1
  end

  # A trailing ACTION is not an option: it DOES something instead of selecting something. So it
  # wears the chip's raised look at rest rather than the dim label of an option waiting to be
  # picked, and it rides inside the menu — last, which puts it at the end of the track and at the
  # foot of the collapsed list. No segmented-target: the controller must never measure it, park the
  # chip on it, or arrow onto it.
  test 'a trailing action is the menu last child and wears its own class' do
    render_segmented(options: buttons, action: { href: '/connect', label: 'Connect exchange',
                                                 icon: 'svg/24x24_plus', data: { turbo_frame: 'modal' } })

    assert_select '.segmented__menu > *:last-child.segmented__option--action', 1
    assert_select "a.segmented__option--action[href='/connect'][data-turbo-frame='modal']", 1
    assert_select ".segmented__option--action[aria-label='Connect exchange']", 1
    assert_select '.segmented__option--action', text: ''
  end

  test 'an action is invisible to the controller' do
    render_segmented(options: buttons, action: { href: '/connect', label: 'Connect exchange',
                                                 icon: 'svg/24x24_plus' })

    assert_select '.segmented__option--action[data-segmented-target]', 0
    assert_select '.segmented__option--action[data-action]', 0
    assert_select '[data-segmented-target="option"]', 2
  end

  # Glyph only in the track — it is read in the company of the row it adds to. A screen reader has
  # no such company, so the name has to be on the element.
  test 'an action carries its icon and takes its name from the attribute' do
    render_segmented(options: buttons, action: { href: '/connect', label: 'Connect exchange',
                                                 icon: 'svg/24x24_plus' })

    assert_select '.segmented__option--action svg', 1
    assert_select ".segmented__option--action[aria-label='Connect exchange']", 1
  end

  test 'no action, no extra child' do
    render_segmented(options: buttons)

    assert_select '.segmented__option--action', 0
    assert_select '.segmented__menu > *', 2
  end
end
