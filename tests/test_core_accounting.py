import unittest

from sic_core.accounting import (
    normalize_usage, summarize_records, validate_categories, timing_summary,
    representation_metrics, controlled_cost, compare_workflows,
)


def measured_resources():
    return [{'record_id': name, 'resource': name, 'quantity': 0, 'unit': 'fixture-unit',
             'unit_price': '0', 'currency': 'USD', 'price_version': 'fixture-v1',
             'measurement_method': 'explicit synthetic zero measurement'}
            for name in ('program', 'storage')]


class AccountingTests(unittest.TestCase):
    def test_openai_totals_split_without_double_counting(self):
        raw = {'prompt_tokens': 100, 'completion_tokens': 40, 'total_tokens': 140,
               'prompt_tokens_details': {'cached_tokens': 60, 'cache_write_tokens': 10},
               'completion_tokens_details': {'reasoning_tokens': 15}, 'extra': {'retained': True}}
        result = normalize_usage(raw)
        self.assertEqual({x['category']: x['quantity'] for x in result['categories']},
                         {'input_uncached': 30, 'cache_read': 60, 'cache_write': 10,
                          'output_nonreasoning': 25, 'output_reasoning': 15})
        self.assertEqual(result['raw_usage'], raw)
        result['raw_usage']['extra']['retained'] = False
        self.assertTrue(raw['extra']['retained'])

    def test_responses_usage_uses_its_own_field_names(self):
        result = normalize_usage({'input_tokens': 10, 'output_tokens': 3,
                                  'input_tokens_details': {'cached_tokens': 4},
                                  'output_tokens_details': {'reasoning_tokens': 2}}, 'openai-responses')
        self.assertEqual([x['quantity'] for x in result['categories']], [6, 4, 1, 2])

    def test_anthropic_cache_is_not_subtracted_from_regular_input(self):
        result = normalize_usage({'input_tokens': 10, 'output_tokens': 8,
                                  'cache_read_input_tokens': 50, 'cache_creation_input_tokens': 30,
                                  'cache_creation': {'ephemeral_5m_input_tokens': 20,
                                                     'ephemeral_1h_input_tokens': 10}}, 'anthropic-messages')
        quantities = {x['category']: x['quantity'] for x in result['categories']}
        self.assertEqual(quantities, {'input_uncached': 10, 'cache_read': 50,
                                    'cache_write_5m': 20, 'cache_write_1h': 10, 'output_unsplit': 8})

    def test_missing_usage_and_cache_split_remain_unknown(self):
        self.assertEqual(normalize_usage(None)['usage_status'], 'unknown')
        result = normalize_usage({'prompt_tokens': 10, 'completion_tokens': 2})
        self.assertEqual(result['usage_status'], 'partial')
        self.assertIn('input_cache_split', result['unknown_components'])
        unified = normalize_usage({'prompt_tokens': 10, 'completion_tokens': 2},
                                  policy={'input_billing': 'unified'})
        self.assertEqual(unified['categories'][0]['category'], 'input_total')

    def test_impossible_usage_is_rejected(self):
        for raw in [{'prompt_tokens': -1}, {'prompt_tokens': True},
                    {'prompt_tokens': 3, 'prompt_tokens_details': {'cached_tokens': 4}},
                    {'completion_tokens': 2, 'completion_tokens_details': {'reasoning_tokens': 3}},
                    {'prompt_tokens': 3, 'completion_tokens': 2, 'total_tokens': 8}]:
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                normalize_usage(raw)

    def test_aggregate_and_subcategory_overlap_is_rejected(self):
        for names in [('input_total', 'cache_read'), ('output_total', 'output_reasoning'),
                      ('cache_write', 'cache_write_5m'), ('input_total', 'input_cache_split_unknown')]:
            with self.subTest(names=names), self.assertRaises(ValueError):
                validate_categories([{'category': n, 'quantity': 1} for n in names])

    def test_decimal_cost_and_versioned_model_prices(self):
        calls = [{'call_id': 'a', 'model': 'first', 'categories': [{'category': 'input_total', 'quantity': 3}],
                  'usage_status': 'provider_reported', 'price_version': 'v1'},
                 {'call_id': 'b', 'model': 'second', 'categories': [{'category': 'input_total', 'quantity': 4}],
                  'usage_status': 'provider_reported', 'price_version': 'v1'}]
        prices = [{'category': 'input_total', 'model': m, 'unit_price': p, 'currency': 'USD',
                   'price_version': 'v1', 'unit': 'token'} for m, p in [('first', '0.1'), ('second', '0.2')]]
        report = summarize_records(calls, measured_resources(), prices=prices, task_interval=[0, 1])
        self.assertEqual(report['total_cost'], '1.1')
        self.assertTrue(report['complete_cost'])
        calls[1]['price_version'] = 'v2'
        report = summarize_records(calls, measured_resources(), prices=prices)
        self.assertFalse(report['complete_cost'])
        self.assertIsNone(report['total_cost'])

    def test_unknown_attempt_or_resource_cannot_become_complete_zero_cost(self):
        report = summarize_records([{'call_id': 'failed', 'categories': [], 'usage_status': 'unknown'}])
        self.assertFalse(report['complete_cost'])
        self.assertIsNone(report['total_cost'])
        reasons = {x['reason'] for x in report['unknown']}
        self.assertIn('usage_incomplete', reasons)
        self.assertIn('resource_measurement_missing', reasons)

    def test_multiple_currencies_are_not_summed(self):
        calls = [{'call_id': currency, 'categories': [{'category': 'input_total', 'quantity': 1,
                  'unit_price': '2', 'price_version': 'v1', 'currency': currency}]} for currency in ('USD', 'CNY')]
        report = summarize_records(calls, required_resources=())
        self.assertEqual(report['totals_by_currency'], {'USD': '2', 'CNY': '2'})
        self.assertFalse(report['complete_cost'])
        self.assertIsNone(report['total_cost'])

    def test_duplicate_calls_and_invalid_price_denominator_fail(self):
        with self.assertRaises(ValueError):
            summarize_records([{'call_id': 'x'}, {'call_id': 'x'}])
        with self.assertRaises(ValueError):
            summarize_records([{'call_id': 'x', 'categories': [{'category': 'input_total', 'quantity': 1,
                               'unit_price': '2', 'currency': 'USD', 'price_version': 'v1', 'per_quantity': 0}]}])

    def test_parallel_intervals_are_unioned_and_overlap_is_intersected(self):
        calls = [{'call_id': 'e1', 'role': 'editor', 'started_at': 1, 'ended_at': 5},
                 {'call_id': 'e2', 'role': 'editor', 'started_at': 3, 'ended_at': 7},
                 {'call_id': 'w', 'role': 'worker', 'started_at': 7, 'ended_at': 9}]
        events = [{'kind': 'tool', 'start': 2, 'end': 6}, {'kind': 'tool', 'start': 4, 'end': 8},
                  {'kind': 'serial_wait', 'start': 8, 'end': 9}]
        result = timing_summary(calls, events, [0, 10])
        self.assertEqual(result['model_cumulative_seconds'], 10)
        self.assertEqual(result['model_union_seconds'], 8)
        self.assertEqual(result['editor_tool_overlap_seconds'], 5)
        self.assertEqual(result['tool_union_seconds'], 6)
        self.assertEqual(result['task_wall_seconds'], 10)
        with self.assertRaises(ValueError):
            timing_summary(calls, events, [0, 4])

    def test_missing_timestamps_do_not_fabricate_overlap(self):
        result = timing_summary([{'call_id': 'e', 'role': 'editor', 'call_duration_seconds': 3}])
        self.assertEqual(result['model_cumulative_seconds'], 3)
        self.assertIsNone(result['editor_tool_overlap_seconds'])
        self.assertIsNone(result['task_wall_seconds'])

    def test_keep_body_size_and_billed_output_are_separate(self):
        result = representation_metrics(wb_body='abcdefghij', cb_body='abcdefghij',
                                        persistent_operations='KEEP', presented_index='[]',
                                        editor_billed_output=1, count=len,
                                        counter_id='fixture-codepoint-v1', tokenizer_model='fixture-not-model-tokens')
        self.assertEqual(result['rho_content'], '1')
        self.assertEqual(result['rho_billed'], '0.1')
        self.assertEqual(result['background_count'], '16')
        grown = representation_metrics(wb_body='a', cb_body='abc', persistent_operations='',
                                       presented_index='', editor_billed_output=None, count=len,
                                       counter_id='fixture-codepoint-v1', tokenizer_model='fixture-not-model-tokens')
        self.assertEqual(grown['rho_content'], '3')
        self.assertIsNone(grown['rho_billed'])

    def test_controlled_cost_and_ledger_reconciliation(self):
        terms = {'background_counts': [10, 20, 30], 'p_u': 2, 'p_h': 1, 'p_o': 3,
                 'O_S': 5, 'O_B': 8, 'S_W': 40, 'D': 7, 'currency': 'USD',
                 'assumptions': {x: True for x in ('same_source_read_once', 'same_editor_model',
                     'same_source_input_pricing', 'first_background_uncached', 'later_background_cached')}}
        result = controlled_cost(terms)
        self.assertEqual((result['F'], result['H'], result['delta_cost']), ('30', '10', '28'))
        base = {'complete_cost': True, 'total_cost': '100', 'currency': 'USD',
                'timing': {'task_wall_seconds': 10}}
        sic = {'complete_cost': True, 'total_cost': '128', 'currency': 'USD',
               'timing': {'task_wall_seconds': 7}}
        comparison = compare_workflows(base, sic, terms)
        self.assertEqual(comparison['delta_wall_seconds'], -3)
        self.assertTrue(comparison['controlled_comparison']['reconciled_with_ledger'])
        self.assertEqual(comparison['quality']['status'], 'not_scored')
        terms['assumptions']['later_background_cached'] = False
        self.assertFalse(controlled_cost(terms)['applicable'])


if __name__ == '__main__':
    unittest.main()
