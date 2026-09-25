// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// Served from GitHub Pages as a project site: https://new-tricks.github.io/tricks/
export default defineConfig({
	site: 'https://new-tricks.github.io',
	base: '/tricks',
	integrations: [
		starlight({
			title: 'New Tricks',
			description: 'The design-time workbench for agent skills: discover, customize, test with real agents, lint and publish.',
			logo: { src: './src/assets/logo.svg' },
			favicon: '/favicon.svg',
			social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/new-tricks/tricks' }],
			editLink: { baseUrl: 'https://github.com/new-tricks/tricks/edit/main/docs/' },
			customCss: ['./src/styles/custom.css'],
			lastUpdated: true,
			sidebar: [
				{
					label: 'Start here',
					items: ['getting-started/introduction', 'getting-started/installation', 'getting-started/quick-start'],
				},
				{
					label: 'Concepts',
					items: [
						'concepts/source-repo',
						'concepts/discovery',
						'concepts/links-and-trials',
						'concepts/branch-experiments',
						'concepts/upstream',
						'concepts/lint',
						'concepts/publishing',
						'concepts/agents',
					],
				},
				{
					label: 'Guides',
					items: ['guides/customize-an-upstream-skill', 'guides/test-a-draft-with-agents', 'guides/working-with-coding-agents'],
				},
				{ label: 'VS Code extension', link: '/vscode/' },
				{
					label: 'Reference',
					items: [
						{ label: 'Commands', collapsed: true, items: [{ autogenerate: { directory: 'reference/commands' } }] },
						'reference/configuration',
						'reference/lint-rules',
					],
				},
			],
		}),
	],
});
